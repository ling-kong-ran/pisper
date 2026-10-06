//! Tests for the ported `coding-agent/src/core/model-registry.ts`: the
//! synchronous compatibility facade over [`ModelRuntime`], over the
//! reachable upstream `model-registry.test.ts` scenarios (the
//! agent-session/settings-dependent describes are not reachable in this
//! slice).

use std::sync::Arc;

use crate::ai::auth::credential_store::InMemoryCredentialStore;
use crate::ai::types::ModelInput;
use crate::coding_agent::core::model_registry::{
    clear_api_key_cache, ModelRegistry, ResolvedRequestAuth,
};
use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::provider_composer::ProviderConfigInput;

/// `createModelRegistry(credentials, modelsPath)` (the upstream test
/// helper): optional models.json, in-memory store, no network.
async fn create_registry(providers: serde_json::Value) -> ModelRegistry {
    let dir = tempfile::TempDir::with_prefix("pi-model-registry-").unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, serde_json::to_string(&providers).unwrap()).unwrap();
    let _ = dir.keep();
    let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(InMemoryCredentialStore::default())),
        models_path: Some(Some(path.to_str().unwrap().to_string())),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .unwrap();
    ModelRegistry::new(runtime)
}

fn raw_providers(providers: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "providers": providers })
}

async fn models_for(registry: &ModelRegistry, provider: &str) -> Vec<crate::ai::types::Model> {
    registry
        .get_all()
        .await
        .into_iter()
        .filter(|model| model.provider == provider)
        .collect()
}

/// Upstream "overriding baseUrl keeps all built-in models" and "changes URL
/// on all built-in models" through the facade.
#[tokio::test]
async fn facade_reports_base_url_overrides() {
    let registry = create_registry(raw_providers(serde_json::json!({
        "anthropic": { "baseUrl": "https://my-proxy.example.com/v1" },
    })))
    .await;

    let anthropic = models_for(&registry, "anthropic").await;
    assert!(anthropic.len() > 1);
    assert!(anthropic.iter().any(|model| model.id.contains("claude")));
    for model in &anthropic {
        assert_eq!(model.base_url, "https://my-proxy.example.com/v1");
    }
    assert!(registry.get_error().is_none());
}

/// Upstream "custom provider with same name as built-in merges with
/// built-in models" + "provider-level baseUrl applies to both built-in and
/// custom models".
#[tokio::test]
async fn facade_reports_custom_model_merges() {
    let registry = create_registry(raw_providers(serde_json::json!({
        "anthropic": {
            "baseUrl": "https://merged-proxy.example.com/v1",
            "apiKey": "test-key",
            "api": "anthropic-messages",
            "models": [{"id": "claude-custom", "reasoning": false, "input": ["text"]}],
        },
    })))
    .await;

    let anthropic = models_for(&registry, "anthropic").await;
    assert!(anthropic.len() > 1);
    assert!(anthropic.iter().any(|model| model.id == "claude-custom"));
    for model in &anthropic {
        assert_eq!(model.base_url, "https://merged-proxy.example.com/v1");
    }

    // `find` locates the custom model through the facade.
    let found = registry.find("anthropic", "claude-custom").await.unwrap();
    assert_eq!(found.provider, "anthropic");
}

/// Upstream "headers-only override resolves at request time" via
/// `getApiKeyAndHeaders` (the compatibility path: no configured provider
/// auth → ok with headers only).
#[tokio::test]
async fn facade_resolves_headers_at_request_time() {
    let registry = create_registry(raw_providers(serde_json::json!({
        "anthropic": {
            "headers": { "X-Custom-Header": "custom-value" },
        },
    })))
    .await;

    assert!(registry.get_error().is_none());
    let models = models_for(&registry, "anthropic").await;
    for model in &models {
        let auth = registry.get_api_key_and_headers(model).await;
        match &auth {
            ResolvedRequestAuth::Ok { headers, .. } => {
                assert_eq!(
                    headers
                        .as_ref()
                        .and_then(|headers| headers.get("X-Custom-Header")),
                    Some(&Some("custom-value".to_string())),
                );
            }
            ResolvedRequestAuth::Err { error } => panic!("unexpected error: {error}"),
        }
    }
}

/// Upstream "unconfigured compatibility auth includes static model
/// headers": an unknown provider with model headers resolves ok with exactly
/// those headers.
#[tokio::test]
async fn facade_compatibility_auth_includes_static_model_headers() {
    let registry = create_registry(raw_providers(serde_json::json!({}))).await;
    let base = registry.get_all().await.remove(0);
    let mut model = base.clone();
    model.provider = "missing-provider".to_string();
    model.headers = Some(
        [(
            "X-Static-Model".to_string(),
            Some("static-value".to_string()),
        )]
        .into_iter()
        .collect(),
    );

    let auth = registry.get_api_key_and_headers(&model).await;
    match auth {
        ResolvedRequestAuth::Ok {
            headers,
            api_key,
            base_url,
            env,
        } => {
            assert_eq!(api_key, None);
            assert_eq!(base_url, None);
            assert_eq!(env, None);
            assert_eq!(
                headers.map(|headers| headers.get("X-Static-Model").cloned()),
                Some(Some(Some("static-value".to_string())))
            );
        }
        ResolvedRequestAuth::Err { error } => panic!("unexpected error: {error}"),
    }
}

/// Upstream "getProviderDisplayName resolves registered, built-in, and
/// fallback names" and "getRegisteredProviderIds".
#[tokio::test]
async fn facade_display_names_and_registration_surface() {
    let registry = create_registry(raw_providers(serde_json::json!({}))).await;

    // Built-in names come from the generated catalog; unknown providers
    // fall back to the id.
    let openai = registry.get_provider_display_name("openai").await;
    assert_eq!(openai, "OpenAI");
    let unknown = registry.get_provider_display_name("unknown-provider").await;
    assert_eq!(unknown, "unknown-provider");

    registry
        .register_provider(
            "named-provider",
            ProviderConfigInput {
                name: Some("Named Provider".to_string()),
                base_url: Some("https://provider.test/v1".to_string()),
                api_key: Some("test-key".to_string()),
                api: Some("openai-completions".to_string()),
                models: Some(vec![
                    crate::coding_agent::core::provider_composer::ExtensionModelDefinition {
                        id: "demo-model".to_string(),
                        name: "Demo Model".to_string(),
                        api: None,
                        base_url: None,
                        reasoning: false,
                        thinking_level_map: None,
                        input: vec![ModelInput::Text],
                        cost: Default::default(),
                        context_window: 128_000,
                        max_tokens: 4096,
                        sampling_params: None,
                        sampling_params_by_thinking_level: None,
                        headers: None,
                        compat: None,
                    },
                ]),
                ..ProviderConfigInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        registry.get_provider_display_name("named-provider").await,
        "Named Provider"
    );
    assert!(registry
        .get_registered_provider_ids()
        .iter()
        .any(|id| id == "named-provider"));
    assert!(registry
        .get_registered_native_provider("named-provider")
        .is_none());

    // The api-key config resolves through the composed auth
    // (upstream `getApiKeyForProvider`).
    assert_eq!(
        registry
            .get_api_key_for_provider("named-provider")
            .await
            .as_deref(),
        Some("test-key")
    );

    registry.unregister_provider("named-provider").await;
    assert!(!registry
        .get_registered_provider_ids()
        .iter()
        .any(|id| id == "named-provider"));
}

/// Upstream `registerProvider` by-object form (`registerProvider(provider)`)
/// registers a native provider untouched.
#[tokio::test]
async fn facade_registers_native_providers() {
    let registry = create_registry(raw_providers(serde_json::json!({}))).await;

    struct NativeProvider;
    impl crate::ai::models::Provider for NativeProvider {
        fn id(&self) -> &str {
            "native"
        }
        fn name(&self) -> &str {
            "Native"
        }
        fn auth(&self) -> &crate::ai::auth::types::ProviderAuth {
            static AUTH: std::sync::OnceLock<crate::ai::auth::types::ProviderAuth> =
                std::sync::OnceLock::new();
            AUTH.get_or_init(|| crate::ai::auth::types::ProviderAuth {
                api_key: Some(Arc::new(EmptyKeyAuth)),
                oauth: None,
            })
        }
        fn get_models(
            &self,
        ) -> Result<Vec<crate::ai::types::Model>, crate::ai::auth::resolve::ModelsError> {
            Ok(vec![crate::ai::types::Model {
                r#type: None,
                prompt_cache: None,
                input_limits: None,
                id: "native-model".to_string(),
                name: "Native Model".to_string(),
                api: "openai-completions".to_string(),
                provider: "native".to_string(),
                base_url: "https://native.test/v1".to_string(),
                reasoning: false,
                thinking_level_map: None,
                input: vec![ModelInput::Text],
                cost: Default::default(),
                context_window: 1000,
                max_tokens: 100,
                sampling_params: None,
                sampling_params_by_thinking_level: None,
                headers: None,
                compat: None,
            }])
        }
    }

    struct EmptyKeyAuth;
    impl crate::ai::auth::types::ApiKeyAuth for EmptyKeyAuth {
        fn name(&self) -> &str {
            "API key"
        }
        fn resolve<'a>(
            &'a self,
            input: crate::ai::auth::types::ApiKeyAuthInput<'a>,
        ) -> futures::future::BoxFuture<
            'a,
            Result<Option<crate::ai::auth::types::AuthResult>, crate::ai::auth::types::AuthError>,
        > {
            Box::pin(async move {
                Ok(input
                    .credential
                    .and_then(|credential| credential.key.clone())
                    .map(|key| crate::ai::auth::types::AuthResult {
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

    registry
        .register_native_provider(Arc::new(NativeProvider))
        .await
        .unwrap();
    assert!(registry.get_registered_native_provider("native").is_some());
    let model = registry.find("native", "native-model").await.unwrap();
    assert_eq!(model.name, "Native Model");
}

/// `clearApiKeyCache` re-exports the config-value cache clear; calling it
/// is observable only through the shared cache, so this witnesses the
/// re-export wiring.
#[test]
fn clear_api_key_cache_is_wired() {
    clear_api_key_cache();
}
