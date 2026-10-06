//! Tests for the ported `coding-agent/src/core/model-runtime.ts`, over the
//! reachable upstream scenarios (`model-registry.test.ts` and
//! `model-runtime-*.test.ts` subsets that do not depend on the unported
//! agent-session/settings modules). Models.json fixtures load through the
//! real [`ModelConfig`]; credentials are in-memory (upstream
//! `AuthStorage.inMemory`); network refresh is off everywhere (upstream
//! `allowModelNetwork: false`).

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::ai::auth::credential_store::{CredentialStore, InMemoryCredentialStore};
use crate::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthError, AuthOperationOptions,
    AuthResult, AuthType, Credential, ProviderAuth,
};
use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
use crate::ai::models::Provider;
use crate::coding_agent::core::model_runtime::{
    CreateModelRuntimeOptions, ModelRuntime, ProviderOrModel,
};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::provider_composer::{ExtensionModelDefinition, ProviderConfigInput};

/// Write the models.json fixture and create a runtime over it.
async fn runtime_with_models_json(providers: serde_json::Value) -> ModelRuntime {
    let dir = tempfile::TempDir::with_prefix("pi-model-runtime-").unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, serde_json::to_string(&providers).unwrap()).unwrap();
    let path = path.to_str().unwrap().to_string();
    let _ = dir.keep();
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(InMemoryCredentialStore::default())),
        models_path: Some(Some(path)),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .unwrap()
}

fn models_for<'a>(
    models: &'a [crate::ai::types::Model],
    provider: &str,
) -> Vec<&'a crate::ai::types::Model> {
    models
        .iter()
        .filter(|model| model.provider == provider)
        .collect()
}

/// Upstream "overriding baseUrl keeps all built-in models" +
/// "changes URL on all built-in models".
#[tokio::test]
async fn base_url_override_keeps_and_rewrites_builtin_models() {
    let runtime = runtime_with_models_json(serde_json::json!({
        "providers": { "anthropic": { "baseUrl": "https://my-proxy.example.com/v1" } }
    }))
    .await;

    assert!(runtime.get_error().is_none());
    let models = runtime.get_models(None).await;
    let anthropic = models_for(&models, "anthropic");
    assert!(
        anthropic.len() > 1,
        "expected multiple built-in anthropic models"
    );
    assert!(
        anthropic.iter().any(|model| model.id.contains("claude")),
        "expected a claude model in the built-in catalog"
    );
    for model in anthropic {
        assert_eq!(model.base_url, "https://my-proxy.example.com/v1");
    }

    // Other providers keep their original baseUrl (upstream
    // "baseUrl-only override does not affect other providers").
    let google = models_for(&models, "google");
    assert!(!google.is_empty());
    assert_ne!(google[0].base_url, "https://my-proxy.example.com/v1");
}

/// Upstream "custom models merge and replace built-ins by id" +
/// "refresh() reloads merged custom models from disk" +
/// "removing custom models keeps built-in provider models".
#[tokio::test]
async fn models_json_merge_replace_and_refresh_round_trip() {
    let dir = tempfile::TempDir::with_prefix("pi-model-runtime-refresh-").unwrap();
    let path = dir.path().join("models.json");
    let write = |providers: serde_json::Value| {
        std::fs::write(&path, serde_json::to_string(&providers).unwrap()).unwrap();
    };
    write(serde_json::json!({
        "providers": { "anthropic": {
            "baseUrl": "https://first-proxy.example.com/v1",
            "models": [{"id": "claude-custom", "reasoning": false, "input": ["text"]}],
        } }
    }));
    let path = path.to_str().unwrap().to_string();
    let _ = dir.keep();
    let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(InMemoryCredentialStore::default())),
        models_path: Some(Some(path.clone())),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .unwrap();

    async fn ids(runtime: &ModelRuntime) -> Vec<String> {
        runtime
            .get_models(None)
            .await
            .iter()
            .filter(|model| model.provider == "anthropic")
            .map(|model| model.id.clone())
            .collect()
    }
    let before = ids(&runtime).await;
    assert!(before.iter().any(|id| id == "claude-custom"), "{before:?}");
    assert!(before.iter().any(|id| id.contains("claude")), "{before:?}");
    // Model-level baseUrl wins over the provider default (upstream
    // "model-level baseUrl overrides provider-level baseUrl").
    let model = runtime
        .get_model("anthropic", "claude-custom")
        .await
        .unwrap();
    assert_eq!(model.base_url, "https://first-proxy.example.com/v1");

    // Update and refresh: the custom model is replaced, the baseUrl moves.
    write(serde_json::json!({
        "providers": { "anthropic": {
            "baseUrl": "https://second-proxy.example.com/v1",
            "models": [{"id": "claude-custom-2", "reasoning": false, "input": ["text"]}],
        } }
    }));
    runtime.refresh(Default::default()).await.unwrap();
    let after = ids(&runtime).await;
    assert!(!after.iter().any(|id| id == "claude-custom"), "{after:?}");
    assert!(after.iter().any(|id| id == "claude-custom-2"), "{after:?}");

    // Removing the custom models restores the plain built-in list.
    write(serde_json::json!({ "providers": {} }));
    runtime.refresh(Default::default()).await.unwrap();
    let restored = ids(&runtime).await;
    assert!(
        !restored.iter().any(|id| id == "claude-custom-2"),
        "{restored:?}"
    );
    assert!(
        restored.iter().any(|id| id.contains("claude")),
        "{restored:?}"
    );
}

/// Upstream "reports every provider composition error" — both broken
/// providers surface in `getError()` with the exact per-provider prefix.
#[tokio::test]
async fn composition_errors_report_every_broken_provider() {
    let runtime = runtime_with_models_json(serde_json::json!({
        "providers": {
            "broken-one": {"api": "openai-completions", "models": [{"id": "one", "reasoning": false, "input": ["text"]}]},
            "broken-two": {"api": "openai-completions", "models": [{"id": "two", "reasoning": false, "input": ["text"]}]},
        }
    }))
    .await;

    let error = runtime.get_error().unwrap();
    assert!(error.contains("Provider \"broken-one\""), "{error}");
    assert!(error.contains("Provider \"broken-two\""), "{error}");
    // The composed provider fell back to the base (deleted for unknown
    // ids), so neither broken provider lists models.
    let models = runtime.get_models(None).await;
    assert!(models_for(&models, "broken-one").is_empty());
    assert!(models_for(&models, "broken-two").is_empty());
}

/// Upstream "non-built-in provider custom models still require baseUrl".
#[tokio::test]
async fn custom_models_on_unknown_providers_require_base_url() {
    let runtime = runtime_with_models_json(serde_json::json!({
        "providers": { "my-custom-provider": {
            "apiKey": "test-key",
            "api": "openai-completions",
            "models": [{"id": "my-model", "api": "openai-completions",
                        "reasoning": false, "input": ["text"]}],
        } }
    }))
    .await;

    let error = runtime.get_error().unwrap();
    assert!(error.contains("baseUrl"), "{error}");
}

/// The dynamic provider lifecycle: register by name (models + overrides),
/// a failed re-registration does not persist or remove models, and
/// unregister removes the config.
#[tokio::test]
async fn register_provider_lifecycle() {
    let runtime = runtime_with_models_json(serde_json::json!({
        "providers": { "extension-provider": {
            "modelOverrides": { "extension-model": { "name": "Overridden Extension Model" } },
        } }
    }))
    .await;

    let config = ProviderConfigInput {
        base_url: Some("https://provider.test/v1".to_string()),
        api_key: Some("test-key".to_string()),
        api: Some("openai-completions".to_string()),
        models: Some(vec![ExtensionModelDefinition {
            id: "extension-model".to_string(),
            name: "Extension Model".to_string(),
            api: None,
            base_url: None,
            reasoning: true,
            thinking_level_map: None,
            input: vec![crate::ai::types::ModelInput::Text],
            cost: Default::default(),
            context_window: 128_000,
            max_tokens: 4096,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }]),
        ..ProviderConfigInput::default()
    };
    runtime
        .register_provider("extension-provider", config.clone())
        .await
        .unwrap();

    // models.json modelOverrides apply on top of the registered models.
    let model = runtime
        .get_model("extension-provider", "extension-model")
        .await
        .unwrap();
    assert_eq!(model.name, "Overridden Extension Model");
    assert_eq!(
        runtime
            .get_registered_provider_config("extension-provider")
            .unwrap()
            .api_key,
        config.api_key
    );
    assert!(runtime
        .get_registered_provider_ids()
        .iter()
        .any(|id| id == "extension-provider"));
    // The provisional auth entry makes the provider available immediately.
    assert!(runtime.has_configured_auth("extension-provider"));
    assert!(runtime
        .get_available_snapshot()
        .iter()
        .any(|model| model.provider == "extension-provider"));

    // A broken re-registration fails without removing the stored models.
    let broken = ProviderConfigInput {
        base_url: Some("https://provider.test/v2".to_string()),
        api_key: Some("test-key".to_string()),
        models: Some(vec![ExtensionModelDefinition {
            id: "broken-model".to_string(),
            name: "Broken Model".to_string(),
            api: None,
            base_url: None,
            reasoning: false,
            thinking_level_map: None,
            input: vec![crate::ai::types::ModelInput::Text],
            cost: Default::default(),
            context_window: 128_000,
            max_tokens: 4096,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }]),
        ..ProviderConfigInput::default()
    };
    let error = runtime
        .register_provider("extension-provider", broken)
        .await
        .unwrap_err();
    assert_eq!(
        error.0,
        "Provider extension-provider, model broken-model: no \"api\" specified. Set at provider or model level."
    );
    assert!(runtime
        .get_model("extension-provider", "extension-model")
        .await
        .is_some());

    // streamSimple without api is rejected upfront (upstream "failed
    // registerProvider does not persist invalid streamSimple config").
    let invalid = ProviderConfigInput {
        stream_simple: Some(Arc::new(|_model, _context, _options| unreachable!())),
        ..ProviderConfigInput::default()
    };
    let error = runtime
        .register_provider("broken-provider", invalid)
        .await
        .unwrap_err();
    assert_eq!(
        error.0,
        "Provider broken-provider: \"api\" is required when registering streamSimple."
    );
    assert!(runtime
        .get_registered_provider_config("broken-provider")
        .is_none());

    // Unregister removes the runtime overlay.
    runtime.unregister_provider("extension-provider").await;
    assert!(runtime
        .get_registered_provider_config("extension-provider")
        .is_none());
}

/// Upstream "registerNativeProvider" + "Provider id must not be empty."
#[tokio::test]
async fn register_native_provider_rejects_blank_ids() {
    let runtime = runtime_with_models_json(serde_json::json!({"providers": {}})).await;
    let provider = create_provider(CreateProviderOptions {
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        id: "   ".to_string(),
        name: None,
        base_url: None,
        headers: None,
        auth: ambient_auth(),
        models: Vec::new(),
        fetch_models: None,
        filter_models: None,
        api: ApiImpls::Single(Arc::new(StubApi)),
    });
    let error = runtime
        .register_native_provider(provider)
        .await
        .unwrap_err();
    assert_eq!(error.0, "Provider id must not be empty.");
}

/// Runtime API keys: set/remove drive `hasConfiguredAuth`, the auth-status
/// source, and the availability snapshot (upstream
/// `setRuntimeApiKey`/`removeRuntimeApiKey`).
#[tokio::test]
async fn runtime_api_keys_drive_configuration_and_availability() {
    let runtime = runtime_with_models_json(serde_json::json!({"providers": {}})).await;
    let provider = dynamic_provider("dynamic");
    runtime.register_native_provider(provider).await.unwrap();
    assert!(!runtime.has_configured_auth("dynamic"));

    runtime
        .set_runtime_api_key("dynamic", "key", None)
        .await
        .unwrap();
    assert!(runtime.has_configured_auth("dynamic"));
    let status = runtime.get_provider_auth_status("dynamic");
    assert!(status.configured);
    assert_eq!(status.source.unwrap().as_str(), "runtime");
    assert!(runtime
        .get_available_snapshot()
        .iter()
        .any(|model| model.provider == "dynamic" && model.id == "dynamic"));

    runtime
        .remove_runtime_api_key("dynamic", None)
        .await
        .unwrap();
    assert!(!runtime.has_configured_auth("dynamic"));
    assert!(!runtime
        .get_available_snapshot()
        .iter()
        .any(|model| model.provider == "dynamic"));
}

/// models.json apiKey configuration feeds the auth-status table (upstream
/// `configuredRequestAuthStatus` through the runtime).
#[tokio::test]
async fn models_json_keys_feed_provider_auth_status() {
    let runtime = runtime_with_models_json(serde_json::json!({
        "providers": { "keyed": {
            "baseUrl": "https://x.example.com/v1",
            "apiKey": "literal-key",
            "api": "openai-completions",
            "models": [{"id": "m1", "reasoning": false, "input": ["text"]}],
        } }
    }))
    .await;
    let status = runtime.get_provider_auth_status("keyed");
    assert!(status.configured);
    assert_eq!(status.source.unwrap().as_str(), "models_json_key");
}

/// Login/logout through the vendored Models surface (upstream
/// model-runtime-credential-sync "publishes locally consistent availability
/// before login and logout resolve").
#[tokio::test]
async fn login_logout_publishes_local_availability() {
    use crate::ai::auth::types::AuthInteraction;

    let credentials = Arc::new(InMemoryCredentialStore::default());
    let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(
            Arc::clone(&credentials) as Arc<dyn crate::ai::auth::credential_store::CredentialStore>
        ),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .unwrap();
    let provider = dynamic_provider("dynamic");
    runtime.register_native_provider(provider).await.unwrap();
    runtime
        .refresh(crate::ai::models::ModelsRefreshOptions {
            allow_network: Some(false),
            providers: Some(vec!["dynamic".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();

    struct PromptInteraction;
    impl AuthInteraction for PromptInteraction {
        fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
            None
        }
        fn prompt(
            &self,
            _prompt: crate::ai::auth::types::AuthPrompt,
        ) -> BoxFuture<'_, Result<String, AuthError>> {
            Box::pin(async { Ok("unused".to_string()) })
        }
        fn notify(&self, _event: crate::ai::auth::types::AuthEvent) {}
    }

    let credential = runtime
        .login("dynamic", AuthType::ApiKey, Arc::new(PromptInteraction))
        .await
        .unwrap();
    assert_eq!(
        credential,
        Credential::ApiKey(ApiKeyCredential {
            key: Some("dynamic-key".to_string()),
            env: None,
            extra: Default::default(),
        })
    );
    assert!(runtime.has_configured_auth("dynamic"));
    assert!(runtime
        .get_available_snapshot()
        .iter()
        .any(|model| model.id == "dynamic"));
    let stored: Option<Credential> = credentials
        .read("dynamic", &AuthOperationOptions::NONE)
        .await
        .unwrap();
    assert_eq!(stored, Some(credential));

    runtime.logout("dynamic", None).await.unwrap();
    assert!(!runtime.has_configured_auth("dynamic"));
    assert!(!runtime
        .get_available_snapshot()
        .iter()
        .any(|model| model.provider == "dynamic"));
    let stored: Option<Credential> = credentials
        .read("dynamic", &AuthOperationOptions::NONE)
        .await
        .unwrap();
    assert_eq!(stored, None);
}

/// `getAuth(model)` layers the composed models.json headers over the
/// resolved auth (upstream model-registry "stored API key env propagates").
#[tokio::test]
async fn get_auth_layers_models_json_headers() {
    let runtime = runtime_with_models_json(serde_json::json!({
        "providers": { "headered": {
            "baseUrl": "https://x.example.com/v1",
            "apiKey": "$RUNTIME_HEADER_KEY",
            "headers": { "x-runtime": "yes" },
            "api": "openai-completions",
            "models": [{"id": "m1", "reasoning": false, "input": ["text"]}],
        } }
    }))
    .await;
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let saved = {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = std::env::var("RUNTIME_HEADER_KEY").ok();
        std::env::set_var("RUNTIME_HEADER_KEY", "resolved-key");
        saved
    };

    let model = runtime.get_model("headered", "m1").await.unwrap();
    let resolution = runtime
        .get_auth(ProviderOrModel::Model(&model), None)
        .await
        .unwrap()
        .unwrap();

    {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match saved {
            Some(value) => std::env::set_var("RUNTIME_HEADER_KEY", value),
            None => std::env::remove_var("RUNTIME_HEADER_KEY"),
        }
    }
    let headers = resolution.auth.headers.unwrap();
    assert_eq!(headers.get("x-runtime"), Some(&Some("yes".to_string())));
    assert_eq!(headers.get("authorization"), None);
    assert_eq!(resolution.auth.api_key.as_deref(), Some("resolved-key"));
}

// ---------------------------------------------------------------------------
// Test doubles (mirroring the upstream credential-sync `provider()` helper)
// ---------------------------------------------------------------------------

fn ambient_auth() -> ProviderAuth {
    struct AmbientKeyAuth;
    impl ApiKeyAuth for AmbientKeyAuth {
        fn name(&self) -> &str {
            "API key"
        }
        fn login<'a>(
            &'a self,
            _interaction: crate::ai::auth::types::ProviderAuthInteraction,
        ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
            Some(Box::pin(async {
                Ok(ApiKeyCredential {
                    key: Some("ambient-key".to_string()),
                    env: None,
                    extra: Default::default(),
                })
            }))
        }
        fn check<'a>(
            &'a self,
            input: ApiKeyAuthInput<'a>,
        ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
            Some(Box::pin(async move {
                Ok(input
                    .credential
                    .and_then(|credential| credential.key.clone())
                    .map(|_| AuthCheck {
                        source: Some("stored".to_string()),
                        r#type: AuthType::ApiKey,
                    }))
            }))
        }
        fn resolve<'a>(
            &'a self,
            input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async move {
                Ok(input
                    .credential
                    .and_then(|credential| credential.key.clone())
                    .map(|key| AuthResult {
                        auth: crate::ai::auth::types::ModelAuth {
                            api_key: Some(key),
                            ..Default::default()
                        },
                        env: None,
                        source: Some("stored".to_string()),
                    }))
            })
        }
    }
    ProviderAuth {
        api_key: Some(Arc::new(AmbientKeyAuth)),
        oauth: None,
    }
}

/// The credential-sync `provider(id)` double: one static model, stored-key
/// check/resolve, and `login` producing `{type: "api_key", key: "<id>-key"}`.
fn dynamic_provider(id: &str) -> Arc<dyn Provider> {
    struct LoginKeyAuth {
        id: String,
    }
    impl ApiKeyAuth for LoginKeyAuth {
        fn name(&self) -> &str {
            "API key"
        }
        fn login<'a>(
            &'a self,
            _interaction: crate::ai::auth::types::ProviderAuthInteraction,
        ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
            let key = format!("{}-key", self.id);
            Some(Box::pin(async move {
                Ok(ApiKeyCredential {
                    key: Some(key),
                    env: None,
                    extra: Default::default(),
                })
            }))
        }
        fn check<'a>(
            &'a self,
            input: ApiKeyAuthInput<'a>,
        ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
            Some(Box::pin(async move {
                Ok(input.credential.map(|_| AuthCheck {
                    source: Some("stored".to_string()),
                    r#type: AuthType::ApiKey,
                }))
            }))
        }
        fn resolve<'a>(
            &'a self,
            input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async move {
                Ok(input
                    .credential
                    .and_then(|credential| credential.key.clone())
                    .map(|key| AuthResult {
                        auth: crate::ai::auth::types::ModelAuth {
                            api_key: Some(key),
                            ..Default::default()
                        },
                        env: None,
                        source: Some("stored".to_string()),
                    }))
            })
        }
    }

    let model = crate::ai::types::Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "dynamic".to_string(),
        name: "Dynamic".to_string(),
        api: "openai-completions".to_string(),
        provider: id.to_string(),
        base_url: "https://example.test/v1".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![crate::ai::types::ModelInput::Text],
        cost: Default::default(),
        context_window: 1000,
        max_tokens: 100,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
    };
    create_provider(CreateProviderOptions {
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        id: id.to_string(),
        name: None,
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(LoginKeyAuth { id: id.to_string() })),
            oauth: None,
        },
        models: vec![crate::ai::types::AnyModel::Chat(model)],
        fetch_models: None,
        filter_models: None,
        api: ApiImpls::Single(Arc::new(StubApi)),
    })
}

/// Eventless API implementation (streams are never dispatched here).
struct StubApi;

impl crate::ai::ApiImpl for StubApi {
    fn stream(
        &self,
        _cfg: &crate::ai::ProviderConfig,
        _model: &crate::ai::types::Model,
        _ctx: &crate::ai::transcript::TranscriptContext,
        _options: &crate::ai::types::options::StreamOptions,
    ) -> tokio::sync::mpsc::Receiver<crate::ai::types::events::AssistantMessageEvent> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        rx
    }

    fn stream_simple(
        &self,
        _cfg: &crate::ai::ProviderConfig,
        _model: &crate::ai::types::Model,
        _ctx: &crate::ai::transcript::TranscriptContext,
        _options: &crate::ai::types::options::SimpleStreamOptions,
    ) -> tokio::sync::mpsc::Receiver<crate::ai::types::events::AssistantMessageEvent> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        rx
    }
}
