//! Tests for the ported `coding-agent/src/core/provider-composer.ts`.
//!
//! Every scenario mirrors `tests/fixtures/core_oracle_model/provider_composer.oracle.json`
//! (the real upstream module under node; generator
//! `oracle_provider_composer.mjs`): the same base-provider stubs, the same
//! models.json fixtures (loaded through the real [`ModelConfig`], like the
//! oracle's typebox-validated `ModelConfig.load`), and normalized JSON
//! comparison (both sides go through [`normalize`], so serde map ordering
//! and `99`/`99.0` number forms agree with the canonical capture).
//!
//! The `pi-ai` stubs the oracle used (lazyStream/getApiProvider throwing)
//! are unreachable in these scenarios; the composed auth flows are driven
//! through a map-backed [`AuthContext`], like the oracle's fake `ctx`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, AuthCheck, AuthContext, AuthError, AuthResult, AuthType,
    ProviderAuth,
};
use crate::ai::models::Provider;
use crate::ai::types::primitives::{ModelCost, ThinkingLevelMap};
use crate::ai::types::{Model, ModelInput};
use crate::coding_agent::core::model_config::{ModelConfig, ModelsJsonProvider};
use crate::coding_agent::core::provider_composer::{
    compose_model_provider, configured_request_auth_status, resolve_compatibility_request_config,
    resolve_configured_model_headers, validate_extension_provider, AuthStatus, AuthStatusSource,
    ExtensionModelDefinition, ExtensionOAuthConfig, ProviderConfigInput,
};
use crate::coding_agent::core::resolve_config_value::ConfigEnv;

/// Upstream provider-composer capture (real upstream module under node;
/// generator `tests/fixtures/core_oracle_model/oracle_provider_composer.mjs`).
const ORACLE: &str =
    include_str!("../../../tests/fixtures/core_oracle_model/provider_composer.oracle.json");

fn oracle() -> serde_json::Value {
    serde_json::from_str(ORACLE).unwrap()
}

/// Normalize a JSON value: all numbers become floats so serde_json's
/// `99.0` compares equal to the oracle's `99` (object key order is already
/// irrelevant for `Value` equality).
fn normalize(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Number(number) => serde_json::Value::Number(
            serde_json::Number::from_f64(number.as_f64().unwrap_or_default()).unwrap(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(normalize).collect())
        }
        serde_json::Value::Object(entries) => entries
            .iter()
            .map(|(key, value)| (key.clone(), normalize(value)))
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into(),
        other => other.clone(),
    }
}

fn assert_json_eq(actual: impl serde::Serialize, expected: &serde_json::Value, label: &str) {
    let actual = normalize(&serde_json::to_value(actual).unwrap());
    let expected = normalize(expected);
    assert_eq!(actual, expected, "{label}");
}

/// JSON.stringify drops `undefined` object values; mirror that for optional
/// fields rendered as `null` on the Rust side.
fn strip_undefined(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(entries) => entries
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, value)| (key.clone(), strip_undefined(value)))
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into(),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(strip_undefined).collect())
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Fixtures (mirroring the oracle script's baseModel/stubProvider)
// ---------------------------------------------------------------------------

fn base_model(provider: &str, id: &str, extra: serde_json::Value) -> Model {
    let extra = extra.as_object().cloned().unwrap_or_default();
    let string = |key: &str| extra.get(key).and_then(|v| v.as_str()).map(String::from);
    let cost = match extra.get("cost") {
        Some(cost) => ModelCost {
            input: cost["input"].as_f64().unwrap(),
            output: cost["output"].as_f64().unwrap(),
            cache_read: cost["cacheRead"].as_f64().unwrap(),
            cache_write: cost["cacheWrite"].as_f64().unwrap(),
            tiers: None,
        },
        None => ModelCost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.1,
            cache_write: 0.2,
            tiers: None,
        },
    };
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: id.to_string(),
        name: string("name").unwrap_or_else(|| id.to_string()),
        api: string("api").unwrap_or_else(|| "openai-completions".to_string()),
        provider: provider.to_string(),
        base_url: string("baseUrl").unwrap_or_else(|| "https://base.example.com/v1".to_string()),
        reasoning: extra
            .get("reasoning")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        thinking_level_map: extra
            .get("thinkingLevelMap")
            .map(|value| serde_json::from_value::<ThinkingLevelMap>(value.clone()).unwrap()),
        input: extra
            .get("input")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|value| match value.as_str() {
                        Some("text") => Some(ModelInput::Text),
                        Some("image") => Some(ModelInput::Image),
                        _ => None,
                    })
                    .collect::<Vec<ModelInput>>()
            })
            .unwrap_or_else(|| vec![ModelInput::Text]),
        cost,
        context_window: extra
            .get("contextWindow")
            .and_then(|v| v.as_u64())
            .unwrap_or(128_000),
        max_tokens: extra
            .get("maxTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(8_192),
        sampling_params: extra.get("samplingParams").map(|value| {
            serde_json::from_value::<BTreeMap<String, serde_json::Value>>(value.clone()).unwrap()
        }),
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: extra.get("compat").cloned(),
    }
}

/// The oracle's duck-typed base provider (always carries `auth: {}` — see
/// the oracle script's stubProvider comment).
struct StubProvider {
    id: String,
    name: String,
    models: Vec<Model>,
    auth: ProviderAuth,
}

impl Provider for StubProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Result<Vec<Model>, crate::ai::auth::resolve::ModelsError> {
        Ok(self.models.clone())
    }
}

fn stub_provider(id: &str, models: Vec<Model>) -> Arc<dyn Provider> {
    Arc::new(StubProvider {
        id: id.to_string(),
        name: id.to_string(),
        models,
        auth: ProviderAuth::default(),
    })
}

fn openrouter_base() -> Arc<dyn Provider> {
    stub_provider(
        "openrouter",
        vec![
            base_model(
                "openrouter",
                "anthropic/claude-sonnet-4",
                serde_json::json!({
                    "name": "Claude Sonnet 4",
                    "reasoning": true,
                    "input": ["text", "image"],
                    "cost": {"input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75},
                    "compat": {"supportsUsageInStreaming": true, "thinkingFormat": "openai",
                               "openRouterRouting": {"allow_fallbacks": true, "order": ["anthropic"]}},
                    "samplingParams": {"temperature": 0.5, "top_p": 0.9},
                }),
            ),
            base_model(
                "openrouter",
                "anthropic/claude-opus-4",
                serde_json::json!({
                    "name": "Claude Opus 4",
                    "reasoning": true,
                    "cost": {"input": 5, "output": 25, "cacheRead": 0.5, "cacheWrite": 6.25},
                    "compat": {"supportsUsageInStreaming": true},
                }),
            ),
            base_model(
                "openrouter",
                "openai/gpt-4o",
                serde_json::json!({"name": "GPT-4o"}),
            ),
        ],
    )
}

/// Write the models.json fixture and load it through the real ModelConfig.
async fn load_config(providers: serde_json::Value) -> ModelConfig {
    let dir = tempfile::TempDir::with_prefix("pi-provider-composer-").unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, serde_json::to_string(&providers).unwrap()).unwrap();
    let _ = dir.keep();
    ModelConfig::load(Some(path.to_str().unwrap()))
        .await
        .unwrap()
}

/// `ModelConfig.getProvider(providerId)` — the composer's `config` argument.
fn provider_slice(config: &ModelConfig, provider_id: &str) -> Option<ModelsJsonProvider> {
    config.get_provider(provider_id).cloned()
}

fn json_provider(api_key: &str) -> ModelsJsonProvider {
    ModelsJsonProvider {
        api_key: Some(api_key.to_string()),
        ..ModelsJsonProvider::default()
    }
}

// ---------------------------------------------------------------------------
// Scenario battery (order mirrors the oracle)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn composed_models_match_the_upstream_oracle() {
    let oracle = oracle();

    // baseUrl_override
    {
        let config = load_config(serde_json::json!({
            "providers": { "openrouter": { "baseUrl": "https://proxy.example.com/v1" } }
        }))
        .await;
        let provider = compose_model_provider(
            "openrouter",
            Some(openrouter_base()),
            provider_slice(&config, "openrouter"),
            None,
        )
        .unwrap();
        let case = &oracle["baseUrl_override"];
        assert_eq!(provider.name(), case["name"].as_str().unwrap());
        assert_eq!(provider.base_url(), case["baseUrl"].as_str());
        assert_json_eq(
            provider.get_models().unwrap(),
            &case["models"],
            "baseUrl_override",
        );
    }

    // headers_only_override (env-dependent header resolution)
    {
        let config = load_config(serde_json::json!({
            "providers": { "openrouter": {
                "headers": { "x-custom": "custom-value", "x-env": "$VAR_X" },
            } }
        }))
        .await;
        let slice = provider_slice(&config, "openrouter");
        let provider =
            compose_model_provider("openrouter", Some(openrouter_base()), slice.clone(), None)
                .unwrap();
        let model = provider.get_models().unwrap()[0].clone();
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = std::env::var("VAR_X").ok();
        std::env::set_var("VAR_X", "env-x");
        let headers =
            resolve_configured_model_headers(&model, slice.as_ref(), None, Some(&ConfigEnv::new()));
        let compat_config = resolve_compatibility_request_config(&model, slice.as_ref(), None);
        match saved {
            Some(value) => std::env::set_var("VAR_X", value),
            None => std::env::remove_var("VAR_X"),
        }
        let actual = normalize(&strip_undefined(&serde_json::json!({
            "value": {
                "compatConfig": {
                    "authHeader": compat_config.as_ref().unwrap().auth_header,
                    "headers": compat_config.as_ref().unwrap().headers,
                },
                "headers": headers
                    .unwrap()
                    .map(
                        |headers: Vec<(String, String)>| serde_json::Value::Object(
                            headers
                                .into_iter()
                                .map(|(key, value)| (key, serde_json::Value::String(value)))
                                .collect(),
                        ),
                    ),
            }
        })));
        let expected = normalize(&serde_json::json!({
            "value": case_value(&oracle, "headers_only_override")
        }));
        assert_eq!(actual, expected, "headers_only_override");
    }

    // models_merge_replace
    {
        let config = load_config(serde_json::json!({
            "providers": { "openrouter": {
                "baseUrl": "https://merged.example.com/v1",
                "models": [
                    {"id": "anthropic/claude-sonnet-4", "name": "Replaced Sonnet", "reasoning": false,
                     "input": ["text"], "cost": {"input": 9, "output": 9, "cacheRead": 0, "cacheWrite": 0},
                     "contextWindow": 1000, "maxTokens": 100},
                    {"id": "brand-new-model", "reasoning": true, "input": ["text"],
                     "thinkingLevelMap": {"high": "high"},
                     "cost": {"input": 1, "output": 2, "cacheRead": 0.3, "cacheWrite": 0.4},
                     "contextWindow": 200000, "maxTokens": 64000,
                     "headers": {"x-model": "m"}, "compat": {"supportsStrictMode": true}},
                ],
            } }
        }))
        .await;
        let provider = compose_model_provider(
            "openrouter",
            Some(openrouter_base()),
            provider_slice(&config, "openrouter"),
            None,
        )
        .unwrap();
        assert_json_eq(
            provider.get_models().unwrap(),
            &oracle["models_merge_replace"]["models"],
            "models_merge_replace",
        );
    }

    // custom_model_inherits_provider_defaults
    {
        let config = load_config(serde_json::json!({
            "providers": { "openrouter": {
                "models": [{"id": "inherit-model", "reasoning": false, "input": ["text"]}],
            } }
        }))
        .await;
        let provider = compose_model_provider(
            "openrouter",
            Some(openrouter_base()),
            provider_slice(&config, "openrouter"),
            None,
        )
        .unwrap();
        assert_json_eq(
            provider.get_models().unwrap(),
            &oracle["custom_model_inherits_provider_defaults"]["models"],
            "custom_model_inherits_provider_defaults",
        );
    }

    // provider_compat_applies_to_models
    {
        let config = load_config(serde_json::json!({
            "providers": { "openrouter": {
                "compat": {"supportsUsageInStreaming": false, "openRouterRouting": {"allow_fallbacks": false}},
            } }
        }))
        .await;
        let provider = compose_model_provider(
            "openrouter",
            Some(openrouter_base()),
            provider_slice(&config, "openrouter"),
            None,
        )
        .unwrap();
        assert_json_eq(
            provider.get_models().unwrap(),
            &oracle["provider_compat_applies_to_models"]["models"],
            "provider_compat_applies_to_models",
        );
    }

    // model_overrides
    {
        let config = load_config(serde_json::json!({
            "providers": { "openrouter": {
                "modelOverrides": {
                    "anthropic/claude-sonnet-4": {
                        "name": "Overridden Sonnet",
                        "reasoning": false,
                        "thinkingLevelMap": {"high": "xhigh", "off": null},
                        "input": ["text"],
                        "cost": {"input": 99},
                        "contextWindow": 555000,
                        "maxTokens": 1234,
                        "samplingParams": {"top_p": 0.7},
                        "compat": {"supportsUsageInStreaming": false,
                                   "openRouterRouting": {"order": ["together"]}},
                    },
                },
            } }
        }))
        .await;
        let provider = compose_model_provider(
            "openrouter",
            Some(openrouter_base()),
            provider_slice(&config, "openrouter"),
            None,
        )
        .unwrap();
        assert_json_eq(
            provider.get_models().unwrap(),
            &oracle["model_overrides"]["models"],
            "model_overrides",
        );
    }

    // override_unknown_model_ignored
    {
        let config = load_config(serde_json::json!({
            "providers": { "openrouter": {
                "modelOverrides": {"no/such-model": {"name": "x"}},
            } }
        }))
        .await;
        let provider = compose_model_provider(
            "openrouter",
            Some(openrouter_base()),
            provider_slice(&config, "openrouter"),
            None,
        )
        .unwrap();
        assert_json_eq(
            provider.get_models().unwrap(),
            &oracle["override_unknown_model_ignored"]["models"],
            "override_unknown_model_ignored",
        );
    }

    // extension_models_replace
    {
        let ext = ProviderConfigInput {
            name: Some("Ext Provider".to_string()),
            base_url: Some("https://ext.example.com/v1".to_string()),
            api_key: Some("ext-key".to_string()),
            api: Some("openai-completions".to_string()),
            models: Some(vec![ExtensionModelDefinition {
                id: "ext-model".to_string(),
                name: "Ext Model".to_string(),
                api: None,
                base_url: None,
                reasoning: true,
                thinking_level_map: None,
                input: vec![ModelInput::Text],
                cost: ModelCost::default(),
                context_window: 4096,
                max_tokens: 512,
                sampling_params: None,
                sampling_params_by_thinking_level: None,
                headers: Some(vec![("x-ext".to_string(), "dropped".to_string())]),
                compat: None,
            }]),
            ..ProviderConfigInput::default()
        };
        let provider =
            compose_model_provider("ext-provider", Some(openrouter_base()), None, Some(ext))
                .unwrap();
        let case = &oracle["extension_models_replace"];
        assert_eq!(provider.name(), case["name"].as_str().unwrap());
        assert_json_eq(
            provider.get_models().unwrap(),
            &case["models"],
            "extension_models_replace",
        );
    }

    // extension_base_url_only
    {
        let ext = ProviderConfigInput {
            base_url: Some("https://ext-overlay.example.com/v1".to_string()),
            ..ProviderConfigInput::default()
        };
        let provider =
            compose_model_provider("openrouter", Some(openrouter_base()), None, Some(ext)).unwrap();
        assert_json_eq(
            provider.get_models().unwrap(),
            &oracle["extension_base_url_only"]["models"],
            "extension_base_url_only",
        );
    }

    // extension_models_merge_with_models_json
    {
        let config = load_config(serde_json::json!({
            "providers": { "openrouter": {
                "baseUrl": "https://json.example.com/v1",
                "models": [{"id": "json-model", "reasoning": false, "input": ["text"]}],
            } }
        }))
        .await;
        let ext = ProviderConfigInput {
            base_url: Some("https://ext.example.com/v1".to_string()),
            api: Some("openai-completions".to_string()),
            models: Some(vec![ExtensionModelDefinition {
                id: "ext-model".to_string(),
                name: "Ext Model".to_string(),
                api: None,
                base_url: None,
                reasoning: false,
                thinking_level_map: None,
                input: vec![ModelInput::Text],
                cost: ModelCost::default(),
                context_window: 4096,
                max_tokens: 512,
                sampling_params: None,
                sampling_params_by_thinking_level: None,
                headers: None,
                compat: None,
            }]),
            ..ProviderConfigInput::default()
        };
        let provider = compose_model_provider(
            "openrouter",
            Some(openrouter_base()),
            provider_slice(&config, "openrouter"),
            Some(ext),
        )
        .unwrap();
        assert_json_eq(
            provider.get_models().unwrap(),
            &oracle["extension_models_merge_with_models_json"]["models"],
            "extension_models_merge_with_models_json",
        );
    }

    // Structural error texts.
    let error_cases: [(&str, &str, serde_json::Value); 6] = [
        (
            "error_missing_api",
            "no-api",
            serde_json::json!({
            "providers": { "no-api": {"baseUrl": "https://x.example.com",
                "models": [{"id": "m1", "reasoning": false, "input": ["text"]}] } } }),
        ),
        (
            "error_missing_base_url",
            "no-url",
            serde_json::json!({
            "providers": { "no-url": {"api": "openai-completions",
                "models": [{"id": "m1", "reasoning": false, "input": ["text"]}] } } }),
        ),
        (
            "error_invalid_context_window",
            "bad-cw",
            serde_json::json!({
            "providers": { "bad-cw": {"baseUrl": "https://x.example.com", "api": "openai-completions",
                "models": [{"id": "m1", "reasoning": false, "input": ["text"], "contextWindow": 0}] } } }),
        ),
        (
            "error_invalid_max_tokens",
            "bad-mt",
            serde_json::json!({
            "providers": { "bad-mt": {"baseUrl": "https://x.example.com", "api": "openai-completions",
                "models": [{"id": "m1", "reasoning": false, "input": ["text"], "maxTokens": -5}] } } }),
        ),
        (
            "error_must_specify_something",
            "empty",
            serde_json::json!({
            "providers": { "empty": {} } }),
        ),
        (
            "error_oauth_requires_base_url",
            "rad",
            serde_json::json!({
            "providers": { "rad": {"oauth": "radius"} } }),
        ),
    ];
    for (key, provider_id, providers) in error_cases {
        let config = load_config(providers).await;
        let error = compose_model_provider(
            provider_id,
            None,
            provider_slice(&config, provider_id),
            None,
        )
        .err()
        .expect("composition must fail");
        assert_eq!(error, oracle[key]["error"].as_str().unwrap(), "{key}");
    }

    // no_auth_method_guard_unreachable: composition succeeds with no models
    // (the guard is dead code upstream — see the module docs).
    {
        let config = load_config(serde_json::json!({
            "providers": { "openrouter": {"baseUrl": "https://x.example.com/v1"} }
        }))
        .await;
        let provider = compose_model_provider(
            "openrouter",
            None,
            provider_slice(&config, "openrouter"),
            None,
        )
        .unwrap();
        let case = &oracle["no_auth_method_guard_unreachable"];
        assert_eq!(provider.name(), case["name"].as_str().unwrap());
        assert_json_eq(
            provider.get_models().unwrap(),
            &case["models"],
            "guard scenario",
        );
    }

    // error_streamsimple_requires_api
    {
        let ext = ProviderConfigInput {
            base_url: Some("https://x.example.com".to_string()),
            stream_simple: Some(Arc::new(|_model, _context, _options| {
                unreachable!("oracle stub")
            })),
            ..ProviderConfigInput::default()
        };
        let error = validate_extension_provider("broken-ext", Some(&openrouter_base()), None, &ext)
            .expect_err("validation must fail");
        assert_eq!(
            error,
            oracle["error_streamsimple_requires_api"]["error"]
                .as_str()
                .unwrap()
        );
    }
}

fn case_value<'a>(oracle: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    &oracle[key]["value"]
}

/// The auth-status table (oracle `authStatus`), including the process-env
/// live cases.
#[test]
fn auth_status_table_matches_the_upstream_oracle() {
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let saved = std::env::var("ORACLE_SET_VAR").ok();
    std::env::remove_var("ORACLE_SET_VAR");

    let oracle = oracle();
    let env_config = json_provider("$ORACLE_SET_VAR");
    let env_missing = json_provider("$ORACLE_MISSING_VAR");
    let command_config = json_provider("!run");
    let template_config = json_provider("pre-$ORACLE_SET_VAR");

    let table: Vec<(&str, Option<AuthStatus>)> = vec![
        (
            "extension_key",
            configured_request_auth_status(
                None,
                Some(&ProviderConfigInput {
                    api_key: Some("literal-key".to_string()),
                    ..ProviderConfigInput::default()
                }),
            ),
        ),
        (
            "models_json_key",
            configured_request_auth_status(Some(&json_provider("json-key")), None),
        ),
        (
            "extension_wins",
            configured_request_auth_status(
                Some(&json_provider("json-key")),
                Some(&ProviderConfigInput {
                    api_key: Some("ext-key".to_string()),
                    ..ProviderConfigInput::default()
                }),
            ),
        ),
        (
            "env_configured",
            configured_request_auth_status(Some(&env_config), None),
        ),
        (
            "env_missing",
            configured_request_auth_status(Some(&env_missing), None),
        ),
        (
            "command",
            configured_request_auth_status(Some(&command_config), None),
        ),
        (
            "template_configured",
            configured_request_auth_status(Some(&template_config), None),
        ),
        ("none", configured_request_auth_status(None, None)),
    ];

    std::env::set_var("ORACLE_SET_VAR", "1");
    let live_table: Vec<(&str, Option<AuthStatus>)> = vec![
        (
            "env_configured_live",
            configured_request_auth_status(Some(&env_config), None),
        ),
        (
            "template_configured_live",
            configured_request_auth_status(Some(&template_config), None),
        ),
    ];
    match saved {
        Some(value) => std::env::set_var("ORACLE_SET_VAR", value),
        None => std::env::remove_var("ORACLE_SET_VAR"),
    }

    let expected: HashMap<&str, &serde_json::Value> = oracle["authStatus"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| (case["label"].as_str().unwrap(), &case["value"]))
        .collect();
    for (label, actual) in table.into_iter().chain(live_table) {
        let actual_json = match &actual {
            Some(status) => serde_json::json!({
                "configured": status.configured,
                "source": status.source.map(|source: AuthStatusSource| source.as_str().to_string()),
                "label": status.label,
            }),
            None => serde_json::Value::Null,
        };
        let mut actual_clean = actual_json;
        if let Some(entries) = actual_clean.as_object_mut() {
            entries.retain(|_, value| !value.is_null());
        }
        let mut expected_clean = expected[label].clone();
        if let Some(entries) = expected_clean.as_object_mut() {
            entries.retain(|_, value| !value.is_null());
        }
        assert_eq!(
            normalize(&actual_clean),
            normalize(&expected_clean),
            "auth status {label}"
        );
    }
}

/// Map-backed [`AuthContext`] (the oracle's fake `ctx`).
struct MapContext {
    env: HashMap<String, String>,
}

impl AuthContext for MapContext {
    fn env<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.env.get(name).cloned() })
    }

    fn file_exists<'a>(&'a self, _path: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { false })
    }
}

/// The composed api-key auth resolution (oracle `composedAuth`): config key
/// templates, header resolution, and `authHeader` →
/// `Authorization: Bearer …`.
#[tokio::test]
async fn composed_api_key_auth_matches_the_upstream_oracle() {
    use crate::ai::auth::types::AuthOperationOptions;

    let oracle = oracle();
    let config = load_config(serde_json::json!({
        "providers": { "p": {
            "baseUrl": "https://p.example.com/v1",
            "apiKey": "json-$ORACLE_AUTH_VAR",
            "headers": {"x-json": "$ORACLE_AUTH_VAR", "x-lit": "v"},
            "authHeader": true,
        } }
    }))
    .await;
    let composed = compose_model_provider("p", None, provider_slice(&config, "p"), None).unwrap();
    let ctx = MapContext {
        env: [("ORACLE_AUTH_VAR".to_string(), "secret-var".to_string())]
            .into_iter()
            .collect(),
    };
    let api_key = composed.auth().api_key.clone().unwrap();
    let options = AuthOperationOptions::NONE;
    let check = api_key
        .check(ApiKeyAuthInput {
            ctx: &ctx,
            credential: None,
            options: &options,
        })
        .unwrap()
        .await
        .unwrap();
    let resolved = api_key
        .resolve(ApiKeyAuthInput {
            ctx: &ctx,
            credential: None,
            options: &options,
        })
        .await
        .unwrap();
    let stored_credential = crate::ai::auth::types::ApiKeyCredential {
        key: Some("stored-key".to_string()),
        env: None,
        extra: Default::default(),
    };
    let with_credential = api_key
        .resolve(ApiKeyAuthInput {
            ctx: &ctx,
            credential: Some(&stored_credential),
            options: &options,
        })
        .await
        .unwrap();
    let env_credential = crate::ai::auth::types::ApiKeyCredential {
        key: Some("stored-key".to_string()),
        env: Some(
            [("ORACLE_AUTH_VAR".to_string(), "cred-env".to_string())]
                .into_iter()
                .collect(),
        ),
        extra: Default::default(),
    };
    let with_env_credential = api_key
        .resolve(ApiKeyAuthInput {
            ctx: &ctx,
            credential: Some(&env_credential),
            options: &options,
        })
        .await
        .unwrap();

    let case = &oracle["composedAuth"];
    assert_eq!(api_key.name(), case["name"].as_str().unwrap());

    let check_json = |check: &Option<AuthCheck>| {
        serde_json::json!({
            "type": check.as_ref().map(|c| serde_json::to_value(c.r#type).unwrap()),
            "source": check.as_ref().and_then(|c| c.source.clone()),
        })
    };
    assert_eq!(
        normalize(&check_json(&check)),
        normalize(&serde_json::json!({
            "type": case["check"]["type"].as_str().map(json_string),
            "source": case["check"]["source"].as_str(),
        })),
        "composedAuth.check"
    );

    let resolve_json = |resolution: &Option<AuthResult>| {
        // The capture keeps `env` as an explicit null (JSON.stringify keeps
        // nulls, drops undefined) and strips the absent `headers`.
        let mut value = serde_json::json!({
            "auth": strip_undefined(&serde_json::json!({
                "apiKey": resolution.as_ref().and_then(|r| r.auth.api_key.clone()),
                "headers": resolution.as_ref().and_then(|r| r.auth.headers.clone()),
            })),
            "env": resolution.as_ref().and_then(|r| r.env.clone()),
            "source": resolution.as_ref().and_then(|r| r.source.clone()),
        });
        if value["auth"]["apiKey"].is_null() {
            value["auth"].as_object_mut().unwrap().remove("apiKey");
        }
        value
    };
    assert_eq!(
        normalize(&resolve_json(&resolved)),
        normalize(&case["resolved"]),
        "composedAuth.resolve"
    );
    assert_eq!(
        normalize(&resolve_json(&with_credential)),
        normalize(&case["withCredential"]),
        "composedAuth.withCredential"
    );
    assert_eq!(
        normalize(&resolve_json(&with_env_credential)),
        normalize(&case["withEnvCredential"]),
        "composedAuth.withEnvCredential"
    );
    assert_eq!(
        composed.auth().oauth.is_some(),
        case["oauthPresent"].as_bool().unwrap()
    );
}

fn json_string(value: &str) -> serde_json::Value {
    serde_json::Value::String(value.to_string())
}

/// Stub provider with the inherited api-key auth (the oracle's
/// `inheritedBase`).
struct AuthedStubProvider {
    id: String,
    name: String,
    models: Vec<Model>,
}

impl Provider for AuthedStubProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn auth(&self) -> &ProviderAuth {
        static INHERITED: std::sync::OnceLock<ProviderAuth> = std::sync::OnceLock::new();
        INHERITED.get_or_init(|| ProviderAuth {
            api_key: Some(Arc::new(InheritedKeyAuth)),
            oauth: None,
        })
    }

    fn get_models(&self) -> Result<Vec<Model>, crate::ai::auth::resolve::ModelsError> {
        Ok(self.models.clone())
    }
}

/// The oracle's `inheritedBase` auth handler.
struct InheritedKeyAuth;

impl ApiKeyAuth for InheritedKeyAuth {
    fn name(&self) -> &str {
        "Inherited Key"
    }

    fn check<'a>(
        &'a self,
        _input: ApiKeyAuthInput<'a>,
    ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
        Some(Box::pin(async move {
            // The oracle's handler ignores the credential entirely.
            Ok(Some(AuthCheck {
                source: Some("inherited check".to_string()),
                r#type: AuthType::ApiKey,
            }))
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            Ok(
                match input
                    .credential
                    .and_then(|credential| credential.key.clone())
                {
                    Some(key) if key == "cred-key" => Some(AuthResult {
                        auth: crate::ai::auth::types::ModelAuth {
                            api_key: Some("cred-key".to_string()),
                            ..Default::default()
                        },
                        env: None,
                        source: Some("from credential".to_string()),
                    }),
                    _ => None,
                },
            )
        })
    }
}

/// The inherited base auth propagation (oracle `inheritedAuth`).
#[tokio::test]
async fn inherited_auth_matches_the_upstream_oracle() {
    use crate::ai::auth::types::AuthOperationOptions;

    let oracle = oracle();
    let case = &oracle["inheritedAuth"];

    let authed_base: Arc<dyn Provider> = Arc::new(AuthedStubProvider {
        id: "inh".to_string(),
        name: "inh".to_string(),
        models: vec![base_model("inh", "m1", serde_json::json!({}))],
    });
    let composed = compose_model_provider("inh", Some(authed_base), None, None).unwrap();
    let ctx = MapContext {
        env: HashMap::new(),
    };
    let api_key = composed.auth().api_key.clone().unwrap();
    let options = AuthOperationOptions::NONE;

    assert_eq!(api_key.name(), case["name"].as_str().unwrap());

    let cred = crate::ai::auth::types::ApiKeyCredential {
        key: Some("cred-key".to_string()),
        env: None,
        extra: Default::default(),
    };
    let check_no = api_key
        .check(ApiKeyAuthInput {
            ctx: &ctx,
            credential: None,
            options: &options,
        })
        .unwrap()
        .await
        .unwrap();
    let check_json = |check: &Option<AuthCheck>| {
        serde_json::json!({
            "type": check.as_ref().map(|c| serde_json::to_value(c.r#type).unwrap()),
            "source": check.as_ref().and_then(|c| c.source.clone()),
        })
    };
    assert_eq!(
        normalize(&check_json(&check_no)),
        normalize(&case["checkNoCredential"]),
        "inherited check without credential"
    );
    let check_with = api_key
        .check(ApiKeyAuthInput {
            ctx: &ctx,
            credential: Some(&cred),
            options: &options,
        })
        .unwrap()
        .await
        .unwrap();
    assert_eq!(
        normalize(&check_json(&check_with)),
        normalize(&case["checkWithCredential"]),
        "inherited check with credential"
    );
    let resolve_no = api_key
        .resolve(ApiKeyAuthInput {
            ctx: &ctx,
            credential: None,
            options: &options,
        })
        .await
        .unwrap();
    assert!(
        resolve_no.is_none(),
        "inherited resolve without credential is undefined"
    );
    let resolve_with = api_key
        .resolve(ApiKeyAuthInput {
            ctx: &ctx,
            credential: Some(&cred),
            options: &options,
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        normalize(&strip_undefined(&serde_json::json!({
            "auth": {
                "apiKey": resolve_with.auth.api_key,
                "headers": resolve_with.auth.headers,
            },
            "source": resolve_with.source,
        }))),
        normalize(&case["resolveWithCredential"]),
        "inherited resolve with credential"
    );
}

/// The oauth-only provider scenario (oracle `oauthOnly`): no fabricated
/// api-key method, adapted OAuth name, and the extension model list.
#[tokio::test]
async fn oauth_only_provider_matches_the_upstream_oracle() {
    use crate::ai::auth::types::OAuthCredential;

    let oracle = oracle();

    let oauth = Arc::new(ExtensionOAuthConfig {
        name: "Ext OAuth".to_string(),
        is_subscription: false,
        uses_callback_server: false,
        login: Arc::new(|_callbacks| {
            Box::pin(async move {
                Ok(OAuthCredential {
                    refresh: "r".to_string(),
                    access: "a".to_string(),
                    expires: 1,
                    extra: Default::default(),
                }) as Result<OAuthCredential, AuthError>
            }) as BoxFuture<'static, Result<OAuthCredential, AuthError>>
        }),
        refresh_token: Arc::new(|credential, _signal| Box::pin(async move { Ok(credential) })),
        get_api_key: Arc::new(|credential| credential.access.clone()),
        modify_models: None,
    });
    let ext = ProviderConfigInput {
        base_url: Some("https://oauth.example.com/v1".to_string()),
        api: Some("openai-completions".to_string()),
        oauth: Some(oauth),
        models: Some(vec![ExtensionModelDefinition {
            id: "oauth-model".to_string(),
            name: "OAuth Model".to_string(),
            api: None,
            base_url: None,
            reasoning: false,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: ModelCost::default(),
            context_window: 4096,
            max_tokens: 512,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }]),
        ..ProviderConfigInput::default()
    };
    let composed = compose_model_provider("oauth-p", None, None, Some(ext)).unwrap();

    let case = &oracle["oauthOnly"];
    assert_eq!(
        composed.auth().api_key.is_some(),
        case["apiKeyPresent"].as_bool().unwrap()
    );
    assert_eq!(
        composed.auth().oauth.as_ref().map(|oauth| oauth.name()),
        Some(case["oauthName"].as_str().unwrap())
    );
    assert_json_eq(
        composed.get_models().unwrap(),
        &case["models"],
        "oauthOnly models",
    );
}

/// Upstream `ModelRegistry` "custom models and model overrides carry sampling
/// params" (model-registry.test.ts @ 200387122) over the composer: the
/// custom model definition carries `samplingParams` +
/// `samplingParamsByThinkingLevel`, and the model override per-level
/// shallow-merges over it (`{ ...base?.[level], ...params }`); models without
/// sampling config keep both fields unset.
#[tokio::test]
async fn custom_models_and_model_overrides_carry_sampling_params_by_thinking_level() {
    let config = load_config(serde_json::json!({
        "providers": { "openrouter": {
            "models": [
                {
                    "id": "custom/sampling-model",
                    "api": "openai-completions",
                    "baseUrl": "https://my-proxy.example.com/v1",
                    "samplingParams": {"temperature": 1, "top_p": 0.95, "top_k": 0},
                    "samplingParamsByThinkingLevel": {
                        "low": {"temperature": 0.6, "top_p": 0.95},
                        "high": {"temperature": 0.8},
                    },
                },
            ],
            "modelOverrides": {
                "custom/sampling-model": {
                    "samplingParamsByThinkingLevel": {
                        "low": {"temperature": 0.5, "top_k": 20},
                        "max": {"temperature": 1},
                    },
                },
                "anthropic/claude-sonnet-4": {
                    "samplingParams": {"top_p": 0.9},
                    "samplingParamsByThinkingLevel": {"high": {"temperature": 0.8}},
                },
            },
        } }
    }))
    .await;
    let provider = compose_model_provider(
        "openrouter",
        Some(openrouter_base()),
        provider_slice(&config, "openrouter"),
        None,
    )
    .unwrap();
    let models = provider.get_models().unwrap();

    let custom = models
        .iter()
        .find(|model| model.id == "custom/sampling-model")
        .unwrap();
    assert_eq!(
        custom.sampling_params.as_ref().unwrap(),
        &BTreeMap::from([
            (String::from("temperature"), serde_json::json!(1)),
            (String::from("top_p"), serde_json::json!(0.95)),
            (String::from("top_k"), serde_json::json!(0)),
        ]),
    );
    assert_eq!(
        custom.sampling_params_by_thinking_level.as_ref().unwrap(),
        &BTreeMap::from([
            (
                String::from("low"),
                BTreeMap::from([
                    (String::from("temperature"), serde_json::json!(0.5)),
                    (String::from("top_p"), serde_json::json!(0.95)),
                    (String::from("top_k"), serde_json::json!(20)),
                ]),
            ),
            (
                String::from("high"),
                BTreeMap::from([(String::from("temperature"), serde_json::json!(0.8))]),
            ),
            (
                String::from("max"),
                BTreeMap::from([(String::from("temperature"), serde_json::json!(1))]),
            ),
        ]),
    );

    let sonnet = models
        .iter()
        .find(|model| model.id == "anthropic/claude-sonnet-4")
        .unwrap();
    // The stub base model carries `{temperature: 0.5, top_p: 0.9}`; the
    // override shallow-merges `{...model.samplingParams, ...override}`.
    assert_eq!(
        sonnet.sampling_params.as_ref().unwrap(),
        &BTreeMap::from([
            (String::from("temperature"), serde_json::json!(0.5)),
            (String::from("top_p"), serde_json::json!(0.9)),
        ]),
    );
    assert_eq!(
        sonnet.sampling_params_by_thinking_level.as_ref().unwrap(),
        &BTreeMap::from([(
            String::from("high"),
            BTreeMap::from([(String::from("temperature"), serde_json::json!(0.8))]),
        )]),
    );

    // Models without sampling config keep both fields unset.
    let opus = models
        .iter()
        .find(|model| model.id == "anthropic/claude-opus-4")
        .unwrap();
    assert_eq!(opus.sampling_params, None);
    assert_eq!(opus.sampling_params_by_thinking_level, None);
}
