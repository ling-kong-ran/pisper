//! Port of upstream `coding-agent/src/core/provider-composer.ts`: compose the
//! built-in provider, models.json, and extension layers into one
//! [`Provider`](crate::ai::models::Provider) without reading credentials.
//!
//! Behavior is pinned against the real upstream module under node in
//! `tests/fixtures/core_oracle_model/provider_composer.oracle.json` (generator
//! `oracle_provider_composer.mjs`): composed `getModels()` lists (canonical
//! key-sorted JSON), every structural error text, the auth-status table, and
//! the composed api-key `resolve` flow (config key templates, header
//! resolution, `authHeader` → `Authorization: Bearer …`).
//!
//! Layering (order matters, all upstream-verbatim): models.json
//! ([`apply_models_json`]) → extension ([`apply_extension`]) →
//! extension-OAuth `modifyModels` → models.json `modelOverrides` (applied
//! last — the topmost user-config layer).
//!
//! Seams:
//! - `AuthStatus`/`ProviderConfigInput`/`ExtensionOAuthConfig` re-exports
//!   move here from `model-registry.ts` exactly like upstream's.
//! - Upstream `Provider` carries `stream`/`streamSimple`; the port's
//!   [`Provider`] routes streams through [`Provider::api_for`], so upstream
//!   `streamWith`'s dispatch becomes `api_for`: the extension `streamSimple`
//!   override (when `model.api` matches the extension api), then the base
//!   provider, then the global API registry ([`get_api_provider`], the port
//!   of `pi-ai/compat`'s `getApiProvider` over the shipped wire APIs).
//! - Upstream `ProviderConfigInput.models` entries carry a
//!   `thinkingLevelMap` spread field and free-form `cost`/`samplingParams`;
//!   the port types them with the ai-layer shapes (`ThinkingLevelMap`,
//!   `ModelCost`) since extensions construct them programmatically.
//! - `composed.refreshModels` publishes through
//!   [`ModelsPublication::update`] like upstream's `context.publish`,
//!   validating the refreshed list before assigning.
//! - Upstream's "no authentication method configured" guard is unreachable
//!   (composeApiKeyAuth only returns `None` when an OAuth method exists, in
//!   which case composeOAuthAuth returns it too); the port keeps the guard
//!   verbatim (dead code, pinned by the oracle capture).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ai::api::anthropic::AnthropicMessages;
use crate::ai::api::azure_openai_responses::AzureOpenAiResponses;
use crate::ai::api::bedrock::BedrockConverseStream;
use crate::ai::api::google_generative_ai::GoogleGenerativeAi;
use crate::ai::api::google_vertex::GoogleVertex;
use crate::ai::api::mistral::MistralConversations;
use crate::ai::api::openai_codex_responses::OpenAiCodexResponses;
use crate::ai::api::openai_completions::OpenAiCompletions;
use crate::ai::api::openai_responses::OpenAiResponses;
use crate::ai::api::pi_messages::PiMessages;
use crate::ai::auth::resolve::{ModelsError, ModelsErrorCode};
use crate::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthError, AuthEvent,
    AuthInteraction, AuthPrompt, AuthPromptKind, AuthPromptOption, AuthResult, AuthType,
    Credential, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuth, ProviderAuthInteraction,
};
use crate::ai::models::{ModelsPublication, Provider, RefreshModelsContext, RefreshModelsError};
use crate::ai::transcript::TranscriptContext;
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::options::{ProviderHeaders, SimpleStreamOptions};
use crate::ai::types::primitives::{
    ModelCost, ModelCostTier, SamplingParamsByThinkingLevel, ThinkingLevelMap,
};
use crate::ai::types::{Model, ModelInput};

use super::model_config::{
    ModelsJsonModel, ModelsJsonModelOverride, ModelsJsonProvider, OrderedValue,
};
use super::resolve_config_value::{
    get_config_value_env_var_names, is_command_config_value, is_config_value_configured,
    resolve_config_value_or_throw, resolve_headers_or_throw, ConfigEnv,
};

/// Ordered `Record<string, string>` (JS object semantics: later spreads
/// overwrite values, the first occurrence keeps the position).
pub type HeaderRecord = Vec<(String, String)>;

// ---------------------------------------------------------------------------
// Extension input types
// ---------------------------------------------------------------------------

/// Upstream `ExtensionOAuthConfig.login`.
pub type ExtensionOAuthLoginFn = Arc<
    dyn Fn(ExtensionOAuthLoginCallbacks) -> BoxFuture<'static, Result<OAuthCredential, AuthError>>
        + Send
        + Sync,
>;

/// Upstream `refreshToken(credentials, signal)`.
pub type ExtensionOAuthRefreshFn = Arc<
    dyn Fn(
            OAuthCredential,
            CancellationToken,
        ) -> BoxFuture<'static, Result<OAuthCredential, AuthError>>
        + Send
        + Sync,
>;

/// Upstream `getApiKey(credentials)`.
pub type ExtensionOAuthGetApiKeyFn = Arc<dyn Fn(&OAuthCredential) -> String + Send + Sync>;

/// Upstream `modifyModels?(models, credentials)`.
pub type ExtensionModifyModelsFn =
    Arc<dyn Fn(Vec<Model>, &OAuthCredential) -> Vec<Model> + Send + Sync>;

/// Upstream `OAuthLoginInfo` (the `onAuth` payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthLoginInfo {
    pub url: String,
    pub instructions: Option<String>,
}

/// Upstream `OAuthDeviceCodeInfo` (the `onDeviceCode` payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthDeviceCodeInfo {
    pub user_code: String,
    pub verification_uri: String,
    pub interval_seconds: Option<u64>,
    pub expires_in_seconds: Option<u64>,
}

/// Upstream `onSelect` prompt payload (option = id, label, description).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthSelectPrompt {
    pub message: String,
    /// `(id, label, description)` triples.
    pub options: Vec<(String, String, Option<String>)>,
}

/// The upstream `OAuthLoginCallbacks` adapter handed to extension login
/// flows: `onAuth`/`onDeviceCode`/`onPrompt`/`onProgress`/
/// `onManualCodeInput`/`onSelect`/`signal`, layered over the canonical
/// [`crate::ai::auth::types::AuthInteraction`].
#[derive(Clone)]
pub struct ExtensionOAuthLoginCallbacks {
    interaction: Arc<dyn crate::ai::auth::types::AuthInteraction>,
}

impl ExtensionOAuthLoginCallbacks {
    pub(crate) fn new(interaction: Arc<dyn crate::ai::auth::types::AuthInteraction>) -> Self {
        Self { interaction }
    }

    /// `onAuth(info)` → `notify({ type: "auth_url", ...info })`.
    pub fn on_auth(&self, info: OAuthLoginInfo) {
        self.interaction.notify(AuthEvent::AuthUrl {
            url: info.url,
            instructions: info.instructions,
        });
    }

    /// `onDeviceCode(info)` → `notify({ type: "device_code", ...info })`.
    pub fn on_device_code(&self, info: OAuthDeviceCodeInfo) {
        self.interaction.notify(AuthEvent::DeviceCode {
            user_code: info.user_code,
            verification_uri: info.verification_uri,
            interval_seconds: info.interval_seconds,
            expires_in_seconds: info.expires_in_seconds,
        });
    }

    /// `onPrompt(prompt)` → `prompt({ type: "text", ...prompt })`.
    pub fn on_prompt(
        &self,
        message: String,
        placeholder: Option<String>,
    ) -> BoxFuture<'_, Result<String, AuthError>> {
        Box::pin(self.interaction.prompt(AuthPrompt {
            signal: None,
            kind: AuthPromptKind::Text {
                message,
                placeholder,
            },
        }))
    }

    /// `onProgress(message)` → `notify({ type: "progress", message })`.
    pub fn on_progress(&self, message: String) {
        self.interaction.notify(AuthEvent::Progress { message });
    }

    /// `onManualCodeInput()` →
    /// `prompt({ type: "manual_code", message: "Paste the authorization code" })`.
    pub fn on_manual_code_input(&self) -> BoxFuture<'_, Result<String, AuthError>> {
        Box::pin(self.interaction.prompt(AuthPrompt {
            signal: None,
            kind: AuthPromptKind::ManualCode {
                message: "Paste the authorization code".to_string(),
                placeholder: None,
            },
        }))
    }

    /// `onSelect(prompt)` → `prompt({ type: "select", ...prompt })`; resolves
    /// with the selected option id.
    pub fn on_select(&self, prompt: OAuthSelectPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        Box::pin(
            self.interaction.prompt(AuthPrompt {
                signal: None,
                kind: AuthPromptKind::Select {
                    message: prompt.message,
                    options: prompt
                        .options
                        .into_iter()
                        .map(|(id, label, description)| AuthPromptOption {
                            id,
                            label,
                            description,
                        })
                        .collect(),
                },
            }),
        )
    }

    /// Upstream `callbacks.signal`.
    pub fn signal(&self) -> CancellationToken {
        self.interaction.signal().unwrap_or_default()
    }
}

/// Upstream `ExtensionOAuthConfig`. `usesCallbackServer` is retained for
/// extension-source compatibility upstream and ignored by canonical auth
/// flows; the port keeps the field for the same reason.
pub struct ExtensionOAuthConfig {
    pub name: String,
    /// Whether access through this auth method is backed by a provider
    /// subscription.
    pub is_subscription: bool,
    /// @deprecated upstream, ignored by canonical auth flows.
    pub uses_callback_server: bool,
    pub login: ExtensionOAuthLoginFn,
    pub refresh_token: ExtensionOAuthRefreshFn,
    pub get_api_key: ExtensionOAuthGetApiKeyFn,
    pub modify_models: Option<ExtensionModifyModelsFn>,
}

/// Upstream extension model entries (`ProviderConfigInput["models"]` item).
#[derive(Clone)]
pub struct ExtensionModelDefinition {
    pub id: String,
    pub name: String,
    pub api: Option<String>,
    pub base_url: Option<String>,
    pub reasoning: bool,
    pub thinking_level_map: Option<ThinkingLevelMap>,
    pub input: Vec<ModelInput>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    pub sampling_params: Option<BTreeMap<String, serde_json::Value>>,
    /// Upstream `ProviderChatModelConfig.samplingParamsByThinkingLevel`.
    pub sampling_params_by_thinking_level: Option<SamplingParamsByThinkingLevel>,
    pub headers: Option<HeaderRecord>,
    pub compat: Option<serde_json::Value>,
}

impl std::fmt::Debug for ExtensionModelDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionModelDefinition")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("api", &self.api)
            .field("base_url", &self.base_url)
            .field("reasoning", &self.reasoning)
            .finish_non_exhaustive()
    }
}

/// Upstream extension `streamSimple`: `(model, context, options) =>
/// AssistantMessageEventStream`, dispatched with request-time auth applied.
pub type ExtensionStreamSimpleFn = Arc<
    dyn Fn(Model, TranscriptContext, SimpleStreamOptions) -> mpsc::Receiver<AssistantMessageEvent>
        + Send
        + Sync,
>;

/// Upstream `refreshModels?(context)`: returns the refreshed model list.
pub type ExtensionRefreshModelsFn = Arc<
    dyn Fn(
            RefreshModelsContext,
        ) -> BoxFuture<'static, Result<Vec<ExtensionModelDefinition>, AuthError>>
        + Send
        + Sync,
>;

/// Upstream `ProviderConfigInput` — the extension `registerProvider` input.
#[derive(Clone, Default)]
pub struct ProviderConfigInput {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api: Option<String>,
    pub stream_simple: Option<ExtensionStreamSimpleFn>,
    pub headers: Option<HeaderRecord>,
    pub auth_header: Option<bool>,
    pub oauth: Option<Arc<ExtensionOAuthConfig>>,
    pub models: Option<Vec<ExtensionModelDefinition>>,
    pub refresh_models: Option<ExtensionRefreshModelsFn>,
}

impl std::fmt::Debug for ProviderConfigInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfigInput")
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "…"))
            .field("api", &self.api)
            .field("stream_simple", &self.stream_simple.is_some())
            .field("headers", &self.headers)
            .field("auth_header", &self.auth_header)
            .field(
                "oauth",
                &self.oauth.as_ref().map(|oauth| oauth.name.clone()),
            )
            .field("models", &self.models.as_ref().map(Vec::len))
            .field("refresh_models", &self.refresh_models.is_some())
            .finish()
    }
}

/// Upstream `AuthStatus["source"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatusSource {
    Stored,
    Runtime,
    Environment,
    Fallback,
    ModelsJsonKey,
    ModelsJsonCommand,
}

impl AuthStatusSource {
    /// The upstream literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthStatusSource::Stored => "stored",
            AuthStatusSource::Runtime => "runtime",
            AuthStatusSource::Environment => "environment",
            AuthStatusSource::Fallback => "fallback",
            AuthStatusSource::ModelsJsonKey => "models_json_key",
            AuthStatusSource::ModelsJsonCommand => "models_json_command",
        }
    }
}

/// Upstream `AuthStatus`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthStatus {
    pub configured: bool,
    pub source: Option<AuthStatusSource>,
    pub label: Option<String>,
}

impl AuthStatus {
    pub(crate) fn configured(source: AuthStatusSource) -> Self {
        Self {
            configured: true,
            source: Some(source),
            label: None,
        }
    }
}

/// Upstream `CompatibilityRequestConfig`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompatibilityRequestConfig {
    pub headers: Option<ProviderHeaders>,
    pub auth_header: bool,
}

// ---------------------------------------------------------------------------
// Ordered-record helpers (JS object spread semantics)
// ---------------------------------------------------------------------------

/// `{ ...base, ...override }` over ordered string records: later values win,
/// the first occurrence keeps the position.
fn spread_records(
    base: Option<&HeaderRecord>,
    override_records: Option<&HeaderRecord>,
) -> Option<HeaderRecord> {
    if base.is_none() && override_records.is_none() {
        return None;
    }
    let mut merged: HeaderRecord = Vec::new();
    for record in [base, override_records].into_iter().flatten() {
        for (key, value) in record {
            match merged.iter_mut().find(|(existing, _)| existing == key) {
                Some(entry) => entry.1 = value.clone(),
                None => merged.push((key.clone(), value.clone())),
            }
        }
    }
    Some(merged)
}

/// `Object.values(record)` (insertion order).
fn record_values(record: &HeaderRecord) -> Vec<String> {
    record.iter().map(|(_, value)| value.clone()).collect()
}

// ---------------------------------------------------------------------------
// OrderedValue → typed extraction
// ---------------------------------------------------------------------------

fn ordered_f64(value: &OrderedValue, key: &str) -> Option<f64> {
    value.get(key).and_then(OrderedValue::as_f64)
}

fn ordered_cost_tiers(value: &OrderedValue) -> Option<Vec<ModelCostTier>> {
    match value.get("tiers") {
        Some(OrderedValue::Array(items)) => Some(
            items
                .iter()
                .map(|tier| ModelCostTier {
                    input: ordered_f64(tier, "input").unwrap_or_default(),
                    output: ordered_f64(tier, "output").unwrap_or_default(),
                    cache_read: ordered_f64(tier, "cacheRead").unwrap_or_default(),
                    cache_write: ordered_f64(tier, "cacheWrite").unwrap_or_default(),
                    input_tokens_above: ordered_f64(tier, "inputTokensAbove").unwrap_or_default()
                        as u64,
                })
                .collect(),
        ),
        _ => None,
    }
}

/// `ModelsJsonModel.cost` / override cost → [`ModelCost`] (missing numbers
/// default to 0, matching the JS consumers of the record fields).
fn ordered_model_cost(value: &OrderedValue) -> ModelCost {
    ModelCost {
        input: ordered_f64(value, "input").unwrap_or_default(),
        output: ordered_f64(value, "output").unwrap_or_default(),
        cache_read: ordered_f64(value, "cacheRead").unwrap_or_default(),
        cache_write: ordered_f64(value, "cacheWrite").unwrap_or_default(),
        tiers: ordered_cost_tiers(value),
    }
}

/// OrderedValue → typed via serde (thinkingLevelMap, samplingParams).
fn ordered_as<T: serde::de::DeserializeOwned>(value: &OrderedValue) -> Option<T> {
    serde_json::from_value(value.to_serde()).ok()
}

/// Validated input literal list (`("text" | "image")[]`).
fn input_list(value: &[String]) -> Option<Vec<ModelInput>> {
    Some(
        value
            .iter()
            .filter_map(|item| match item.as_str() {
                "text" => Some(ModelInput::Text),
                "image" => Some(ModelInput::Image),
                _ => None,
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Model-level composition
// ---------------------------------------------------------------------------

/// Upstream `mergeCompat`: shallow spread plus a deep spread of the four
/// routing/kwarg record keys. Compat stays a raw JSON object (the validated
/// config always carries objects; non-object sides are treated as empty
/// records, which the JS spread would coerce defensively).
fn merge_compat(
    base: Option<&serde_json::Value>,
    override_compat: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    // Upstream: `if (!override) return base;`
    let override_compat = override_compat.cloned().or_else(|| base.cloned())?;
    let override_compat = &override_compat;
    const NESTED_KEYS: [&str; 4] = [
        "openRouterRouting",
        "vercelGatewayRouting",
        "chatTemplateKwargs",
        "chatTemplateArgs",
    ];
    let base_object = base
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    let mut merged = base_object.clone();
    for (key, value) in override_compat.as_object().cloned().unwrap_or_default() {
        merged.insert(key, value);
    }
    for key in NESTED_KEYS {
        let base_value = base_object.get(key);
        let override_value = merged.get(key);
        if base_value.is_some_and(|value| value.is_object())
            || override_value.is_some_and(|value| value.is_object())
        {
            let mut nested = base_value
                .and_then(|value| value.as_object())
                .cloned()
                .unwrap_or_default();
            for (nested_key, nested_value) in override_value
                .and_then(|value| value.as_object())
                .cloned()
                .unwrap_or_default()
            {
                nested.insert(nested_key, nested_value);
            }
            merged.insert(key.to_string(), serde_json::Value::Object(nested));
        }
    }
    Some(serde_json::Value::Object(merged))
}

/// Upstream `mergeSamplingParamsByThinkingLevel`: per-level shallow merge of
/// the override over the base. Levels absent from the override keep the base
/// value (the whole base spreads first); a present level merges
/// `{ ...base?.[level], ...params }` — an empty override object keeps the
/// base level (JS truthiness), and override keys outside the seven canonical
/// `ModelThinkingLevel`s are not read, exactly like upstream's fixed level
/// list.
fn merge_sampling_params_by_thinking_level(
    base: Option<&SamplingParamsByThinkingLevel>,
    override_by_level: Option<&SamplingParamsByThinkingLevel>,
) -> Option<SamplingParamsByThinkingLevel> {
    // Upstream: `if (!override) return base;`
    let override_by_level = override_by_level?;
    const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    let mut merged = base.cloned().unwrap_or_default();
    for level in LEVELS {
        let Some(params) = override_by_level.get(level) else {
            continue;
        };
        let mut level_merged = base
            .and_then(|base| base.get(level))
            .cloned()
            .unwrap_or_default();
        for (key, value) in params {
            level_merged.insert(key.clone(), value.clone());
        }
        merged.insert(String::from(level), level_merged);
    }
    Some(merged)
}

/// Upstream `applyModelOverride` — the topmost user-config layer.
fn apply_model_override(mut model: Model, override_value: &ModelsJsonModelOverride) -> Model {
    if let Some(name) = &override_value.name {
        model.name = name.clone();
    }
    if let Some(reasoning) = override_value.reasoning {
        model.reasoning = reasoning;
    }
    if let Some(thinking_level_map) = &override_value.thinking_level_map {
        // `{ ...model.thinkingLevelMap, ...override.thinkingLevelMap }`.
        let mut merged = model.thinking_level_map.clone().unwrap_or_default();
        for (key, value) in ordered_as::<ThinkingLevelMap>(thinking_level_map).unwrap_or_default() {
            merged.insert(key, value);
        }
        model.thinking_level_map = Some(merged);
    }
    if let Some(input) = override_value.input.as_deref().and_then(input_list) {
        model.input = input;
    }
    if let Some(cost) = &override_value.cost {
        if let Some(input) = ordered_f64(cost, "input") {
            model.cost.input = input;
        }
        if let Some(output) = ordered_f64(cost, "output") {
            model.cost.output = output;
        }
        if let Some(cache_read) = ordered_f64(cost, "cacheRead") {
            model.cost.cache_read = cache_read;
        }
        if let Some(cache_write) = ordered_f64(cost, "cacheWrite") {
            model.cost.cache_write = cache_write;
        }
        if let Some(tiers) = ordered_cost_tiers(cost) {
            model.cost.tiers = Some(tiers);
        }
    }
    if let Some(context_window) = override_value.context_window {
        model.context_window = context_window as u64;
    }
    if let Some(max_tokens) = override_value.max_tokens {
        model.max_tokens = max_tokens as u64;
    }
    if let Some(sampling_params) = &override_value.sampling_params {
        let mut merged = model.sampling_params.clone().unwrap_or_default();
        for (key, value) in
            ordered_as::<BTreeMap<String, serde_json::Value>>(sampling_params).unwrap_or_default()
        {
            merged.insert(key, value);
        }
        model.sampling_params = Some(merged);
    }
    let override_by_level: Option<SamplingParamsByThinkingLevel> = override_value
        .sampling_params_by_thinking_level
        .as_ref()
        .and_then(ordered_as);
    model.sampling_params_by_thinking_level = merge_sampling_params_by_thinking_level(
        model.sampling_params_by_thinking_level.as_ref(),
        override_by_level.as_ref(),
    );
    let override_compat = override_value.compat.as_ref().map(OrderedValue::to_serde);
    model.compat = merge_compat(model.compat.as_ref(), override_compat.as_ref());
    model
}

/// Upstream `modelFromJson` (the structural error texts are byte-pinned).
fn model_from_json(
    provider_id: &str,
    definition: &ModelsJsonModel,
    provider_config: &ModelsJsonProvider,
    defaults: Option<&Model>,
) -> Result<Model, String> {
    let definition_id = &definition.id;
    let api = definition
        .api
        .as_deref()
        .or(provider_config.api.as_deref())
        .or_else(|| defaults.map(|model| model.api.as_str()))
        .map(str::to_string);
    let Some(api) = api else {
        return Err(format!(
            "Provider {provider_id}, model {definition_id}: no \"api\" specified. Set at provider or model level."
        ));
    };
    let base_url = definition
        .base_url
        .as_deref()
        .or(provider_config.base_url.as_deref())
        .or_else(|| defaults.map(|model| model.base_url.as_str()))
        .map(str::to_string);
    let Some(base_url) = base_url else {
        return Err(format!(
            "Provider {provider_id}: \"baseUrl\" is required when defining custom models."
        ));
    };
    if definition
        .context_window
        .is_some_and(|context_window| context_window <= 0.0)
    {
        return Err(format!(
            "Provider {provider_id}, model {definition_id}: invalid contextWindow"
        ));
    }
    if definition
        .max_tokens
        .is_some_and(|max_tokens| max_tokens <= 0.0)
    {
        return Err(format!(
            "Provider {provider_id}, model {definition_id}: invalid maxTokens"
        ));
    }
    Ok(Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: definition.id.clone(),
        name: definition
            .name
            .clone()
            .unwrap_or_else(|| definition.id.clone()),
        api,
        provider: provider_id.to_string(),
        base_url,
        reasoning: definition.reasoning.unwrap_or(false),
        thinking_level_map: definition.thinking_level_map.as_ref().and_then(ordered_as),
        input: definition
            .input
            .as_deref()
            .and_then(input_list)
            .unwrap_or_else(|| vec![ModelInput::Text]),
        cost: definition
            .cost
            .as_ref()
            .map(ordered_model_cost)
            .unwrap_or_default(),
        context_window: definition.context_window.unwrap_or(128_000.0) as u64,
        max_tokens: definition.max_tokens.unwrap_or(16_384.0) as u64,
        sampling_params: definition.sampling_params.as_ref().and_then(ordered_as),
        sampling_params_by_thinking_level: definition
            .sampling_params_by_thinking_level
            .as_ref()
            .and_then(ordered_as),
        headers: None,
        compat: merge_compat(
            provider_config
                .compat
                .as_ref()
                .map(OrderedValue::to_serde)
                .as_ref(),
            definition
                .compat
                .as_ref()
                .map(OrderedValue::to_serde)
                .as_ref(),
        ),
    })
}

/// Upstream `findModelDefaults`.
fn find_model_defaults<'a>(
    models: &'a [Model],
    model_id: &str,
    api: Option<&str>,
) -> Option<&'a Model> {
    models
        .iter()
        .find(|model| model.id == model_id)
        .or_else(|| api.and_then(|api| models.iter().find(|model| model.api == api)))
        .or_else(|| {
            models
                .iter()
                .find(|model| model.api == "openai-completions")
        })
        .or_else(|| models.first())
}

/// Upstream `applyModelsJson`.
fn apply_models_json(
    provider_id: &str,
    base_models: &[Model],
    config: Option<&ModelsJsonProvider>,
) -> Result<Vec<Model>, String> {
    let Some(config) = config else {
        return Ok(base_models.to_vec());
    };
    if config.oauth.is_some() && config.base_url.is_none() {
        return Err(format!(
            "Provider {provider_id}: \"baseUrl\" is required when \"oauth\" is set."
        ));
    }
    let has_overrides = config
        .model_overrides
        .as_ref()
        .is_some_and(|overrides| !overrides.is_empty());
    if config
        .models
        .as_ref()
        .is_none_or(|models| models.is_empty())
        && config.base_url.is_none()
        && config.headers.is_none()
        && config.compat.is_none()
        && !has_overrides
        && config.api_key.is_none()
        && config.oauth.is_none()
        && config.auth_header.is_none()
    {
        return Err(format!(
            "Provider {provider_id}: must specify \"baseUrl\", \"headers\", \"compat\", \"modelOverrides\", or \"models\"."
        ));
    }

    let mut models: Vec<Model> = base_models
        .iter()
        .map(|model| {
            let mut mapped = model.clone();
            // models.json `oauth: "radius"` keeps per-model gateway URLs.
            mapped.base_url = if config.oauth.as_deref() == Some("radius") {
                model.base_url.clone()
            } else {
                config
                    .base_url
                    .clone()
                    .unwrap_or_else(|| model.base_url.clone())
            };
            let config_compat = config.compat.as_ref().map(OrderedValue::to_serde);
            mapped.compat = merge_compat(model.compat.as_ref(), config_compat.as_ref());
            mapped
        })
        .collect();
    for definition in config.models.iter().flatten() {
        let existing_index = models.iter().position(|model| model.id == definition.id);
        let defaults = find_model_defaults(
            &models,
            &definition.id,
            definition.api.as_deref().or(config.api.as_deref()),
        );
        let model = model_from_json(provider_id, definition, config, defaults)?;
        match existing_index {
            Some(index) => models[index] = model,
            None => models.push(model),
        }
    }
    Ok(models)
}

/// Upstream `applyExtension`.
fn apply_extension(
    provider_id: &str,
    models: &[Model],
    config: Option<&ProviderConfigInput>,
) -> Result<Vec<Model>, String> {
    let Some(config) = config else {
        return Ok(models.to_vec());
    };
    let Some(definitions) = &config.models else {
        return Ok(match &config.base_url {
            Some(base_url) => models
                .iter()
                .map(|model| {
                    let mut mapped = model.clone();
                    mapped.base_url = base_url.clone();
                    mapped
                })
                .collect(),
            None => models.to_vec(),
        });
    };
    definitions
        .iter()
        .map(|definition| {
            let definition_id = &definition.id;
            let defaults = find_model_defaults(
                models,
                &definition.id,
                definition.api.as_deref().or(config.api.as_deref()),
            );
            let api = definition
                .api
                .as_deref()
                .or(config.api.as_deref())
                .or_else(|| defaults.map(|model| model.api.as_str()))
                .map(str::to_string);
            let Some(api) = api else {
                return Err(format!(
                    "Provider {provider_id}, model {definition_id}: no \"api\" specified. Set at provider or model level."
                ));
            };
            let base_url = definition
                .base_url
                .as_deref()
                .or(config.base_url.as_deref())
                .or_else(|| defaults.map(|model| model.base_url.as_str()))
                .map(str::to_string);
            let Some(base_url) = base_url else {
                return Err(format!(
                    "Provider {provider_id}: \"baseUrl\" is required when defining custom models."
                ));
            };
            Ok(Model {
                r#type: None,
                prompt_cache: None,
                input_limits: None,
                id: definition.id.clone(),
                name: definition.name.clone(),
                api,
                provider: provider_id.to_string(),
                base_url,
                reasoning: definition.reasoning,
                thinking_level_map: definition.thinking_level_map.clone(),
                input: definition.input.clone(),
                cost: definition.cost.clone(),
                context_window: definition.context_window,
                max_tokens: definition.max_tokens,
                sampling_params: definition.sampling_params.clone(),
                sampling_params_by_thinking_level: definition
                    .sampling_params_by_thinking_level
                    .clone(),
                headers: None,
                compat: definition.compat.clone(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Auth composition
// ---------------------------------------------------------------------------

/// Upstream `adaptOAuth`: the extension OAuth config behind the canonical
/// [`OAuthAuth`] surface.
struct ExtensionOAuthAuth {
    config: Arc<ExtensionOAuthConfig>,
}

impl OAuthAuth for ExtensionOAuthAuth {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn is_subscription(&self) -> bool {
        self.config.is_subscription
    }

    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let callbacks = ExtensionOAuthLoginCallbacks::new(Arc::new(interaction));
            // upstream: `{ ...credential, type: "oauth" }` — the port's
            // canonical credential already carries the oauth tag.
            (self.config.login)(callbacks).await
        })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        let signal = options.signal.clone().unwrap_or_default();
        Box::pin(async move { (self.config.refresh_token)(credential, signal).await })
    }

    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            Ok(ModelAuth {
                api_key: Some((self.config.get_api_key)(&credential)),
                ..ModelAuth::default()
            })
        })
    }
}

/// Upstream `withConfiguredAuth`.
fn with_configured_auth(
    auth: &ModelAuth,
    headers: Option<&HeaderRecord>,
    auth_header: bool,
) -> Result<ModelAuth, String> {
    let mut merged_headers: Option<ProviderHeaders> = None;
    if auth.headers.is_some() || headers.is_some_and(|headers| !headers.is_empty()) {
        let mut merged = auth.headers.clone().unwrap_or_default();
        for (key, value) in headers.into_iter().flatten() {
            // Plain record spread: keys are case-sensitive here, later wins.
            merged.insert(key.clone(), Some(value.clone()));
        }
        merged_headers = Some(merged);
    }
    if auth_header {
        let Some(api_key) = &auth.api_key else {
            return Err("authHeader requires a resolved API key".to_string());
        };
        let mut merged = merged_headers.unwrap_or_default();
        merged.insert(
            "Authorization".to_string(),
            Some(format!("Bearer {api_key}")),
        );
        merged_headers = Some(merged);
    }
    Ok(ModelAuth {
        headers: merged_headers,
        ..auth.clone()
    })
}

/// Upstream `configuredApiKey` (extension key wins).
fn configured_api_key<'a>(
    config: Option<&'a ModelsJsonProvider>,
    extension: Option<&'a ProviderConfigInput>,
) -> Option<&'a str> {
    extension
        .and_then(|extension| extension.api_key.as_deref())
        .or(config.and_then(|config| config.api_key.as_deref()))
}

/// Upstream `configuredHeaders` (`{ ...config.headers, ...extension.headers }`).
fn configured_headers(
    config: Option<&ModelsJsonProvider>,
    extension: Option<&ProviderConfigInput>,
) -> Option<HeaderRecord> {
    spread_records(
        config.and_then(|config| config.headers.as_ref()),
        extension.and_then(|extension| extension.headers.as_ref()),
    )
}

/// Upstream `configContextEnv`: collect the env values the config values
/// reference, seeded with `explicit`.
async fn config_context_env(
    values: &[String],
    ctx: &dyn crate::ai::auth::types::AuthContext,
    explicit: Option<ConfigEnv>,
) -> Option<ConfigEnv> {
    let mut env = explicit.unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for value in values {
        for name in get_config_value_env_var_names(value) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    for name in names {
        if env.contains_key(&name) {
            continue;
        }
        if let Some(value) = ctx.env(&name).await {
            env.insert(name, value);
        }
    }
    if env.is_empty() {
        None
    } else {
        Some(env)
    }
}

/// `ProviderAuthInteraction` is an `Arc` wrapper; clone it for handlers
/// that consume it (upstream passes the same interaction object around).
fn reborrow_interaction(interaction: &ProviderAuthInteraction) -> ProviderAuthInteraction {
    interaction.clone()
}

/// Rebuild an [`ApiKeyAuthInput`] (its fields are plain references, but the
/// struct is not `Copy`); upstream passes the same object to multiple
/// inherited handlers.
fn reborrow_input<'a>(input: &ApiKeyAuthInput<'a>) -> ApiKeyAuthInput<'a> {
    ApiKeyAuthInput {
        ctx: input.ctx,
        credential: input.credential,
        options: input.options,
    }
}

/// The composed api-key auth method (upstream `composeApiKeyAuth`).
pub struct ComposedApiKeyAuth {
    provider_id: String,
    method_name: String,
    inherited: Option<Arc<dyn ApiKeyAuth>>,
    raw_key: Option<String>,
    raw_headers: Option<HeaderRecord>,
    auth_header: bool,
}

impl ComposedApiKeyAuth {
    /// Upstream `check` body.
    async fn check_impl(&self, input: ApiKeyAuthInput<'_>) -> Result<Option<AuthCheck>, AuthError> {
        if let Some(credential) = input.credential {
            if let Some(inherited) = &self.inherited {
                if let Some(check) = inherited.check(reborrow_input(&input)) {
                    return check.await;
                }
            }
            if credential.key.is_some() {
                return Ok(Some(AuthCheck {
                    source: Some("stored credential".to_string()),
                    r#type: AuthType::ApiKey,
                }));
            }
            return Ok(self.resolve_impl(input).await?.map(|resolved| AuthCheck {
                source: resolved.source.clone(),
                r#type: AuthType::ApiKey,
            }));
        }
        if let Some(raw_key) = &self.raw_key {
            if is_command_config_value(raw_key) {
                return Ok(Some(AuthCheck {
                    source: Some("configured API key".to_string()),
                    r#type: AuthType::ApiKey,
                }));
            }
            for name in get_config_value_env_var_names(raw_key) {
                if input.ctx.env(&name).await.is_none() {
                    return Ok(None);
                }
            }
            return Ok(Some(AuthCheck {
                source: Some("configured API key".to_string()),
                r#type: AuthType::ApiKey,
            }));
        }
        if let Some(inherited) = &self.inherited {
            if let Some(check) = inherited.check(reborrow_input(&input)) {
                return check.await;
            }
        }
        Ok(self
            .resolve_impl(reborrow_input(&input))
            .await?
            .map(|resolved| AuthCheck {
                source: resolved.source.clone(),
                r#type: AuthType::ApiKey,
            }))
    }

    /// Upstream `resolve` body.
    async fn resolve_impl(
        &self,
        input: ApiKeyAuthInput<'_>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let mut result: Option<AuthResult> = None;
        if let Some(credential) = input.credential {
            if let Some(inherited) = &self.inherited {
                result = inherited.resolve(reborrow_input(&input)).await?;
            } else if let Some(key) = &credential.key {
                result = Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key.clone()),
                        ..ModelAuth::default()
                    },
                    env: credential.env.clone(),
                    source: Some("stored credential".to_string()),
                });
            }
        } else if let Some(raw_key) = &self.raw_key {
            let env = config_context_env(std::slice::from_ref(raw_key), input.ctx, None).await;
            let key = resolve_config_value_or_throw(
                raw_key,
                &format!("API key for provider \"{}\"", self.provider_id),
                env.as_ref(),
            )
            .map_err(AuthError::Operation)?;
            if let Some(inherited) = &self.inherited {
                let credential = ApiKeyCredential {
                    key: Some(key),
                    env: None,
                    extra: Default::default(),
                };
                let nested = ApiKeyAuthInput {
                    ctx: input.ctx,
                    credential: Some(&credential),
                    options: input.options,
                };
                result = inherited.resolve(nested).await?;
            } else {
                result = Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key),
                        ..ModelAuth::default()
                    },
                    env: None,
                    source: Some("configured API key".to_string()),
                });
            }
        } else if let Some(inherited) = &self.inherited {
            result = inherited.resolve(reborrow_input(&input)).await?;
        }

        let Some(mut result) = result else {
            return Ok(None);
        };
        // `{ ...(input.credential?.env ?? {}), ...(result.env ?? {}) }`.
        let mut explicit_env = ConfigEnv::new();
        if let Some(credential_env) = input
            .credential
            .and_then(|credential| credential.env.as_ref())
        {
            for (name, value) in credential_env {
                explicit_env.insert(name.clone(), value.clone());
            }
        }
        if let Some(result_env) = &result.env {
            for (name, value) in result_env {
                explicit_env.insert(name.clone(), value.clone());
            }
        }
        let header_values = self
            .raw_headers
            .as_ref()
            .map(record_values)
            .unwrap_or_default();
        let header_env = config_context_env(&header_values, input.ctx, Some(explicit_env)).await;
        let headers = resolve_headers_or_throw(
            self.raw_headers.as_deref(),
            &format!("provider \"{}\"", self.provider_id),
            header_env.as_ref(),
        )
        .map_err(AuthError::Operation)?;
        result.auth = with_configured_auth(&result.auth, headers.as_ref(), self.auth_header)
            .map_err(AuthError::Operation)?;
        Ok(Some(result))
    }
}

impl ApiKeyAuth for ComposedApiKeyAuth {
    fn name(&self) -> &str {
        &self.method_name
    }

    /// Upstream `login: inherited?.login ?? (async (interaction) => ({
    /// type: "api_key", key: await interaction.prompt({ type: "secret",
    /// message: "Enter API key" }) }))` — always present.
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        if let Some(inherited) = &self.inherited {
            if let Some(login) = inherited.login(reborrow_interaction(&interaction)) {
                return Some(login);
            }
        }
        Some(Box::pin(async move {
            let key = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Secret {
                        message: "Enter API key".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
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
        Some(Box::pin(async move { self.check_impl(input).await }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move { self.resolve_impl(input).await })
    }
}

/// Upstream `composeApiKeyAuth`.
fn compose_api_key_auth(
    provider_id: &str,
    base: Option<&Arc<dyn Provider>>,
    config: Option<&ModelsJsonProvider>,
    extension: Option<&ProviderConfigInput>,
) -> Option<Arc<dyn ApiKeyAuth>> {
    let inherited = base.and_then(|base| base.auth().api_key.clone());
    let raw_key = configured_api_key(config, extension).map(str::to_string);
    let oauth_present = extension.is_some_and(|extension| extension.oauth.is_some())
        || base.is_some_and(|base| base.auth().oauth.is_some());
    // OAuth-only providers get no fabricated API-key login method.
    if inherited.is_none() && raw_key.is_none() && oauth_present {
        return None;
    }
    let raw_headers = configured_headers(config, extension);
    let auth_header = extension
        .and_then(|extension| extension.auth_header)
        .or(config.and_then(|config| config.auth_header))
        .unwrap_or(false);
    let method_name = inherited
        .as_ref()
        .map(|inherited| inherited.name().to_string())
        .unwrap_or_else(|| "API key".to_string());
    Some(Arc::new(ComposedApiKeyAuth {
        provider_id: provider_id.to_string(),
        method_name,
        inherited,
        raw_key,
        raw_headers,
        auth_header,
    }))
}

/// The composed OAuth auth method (upstream `composeOAuthAuth`): the base or
/// adapted extension flow with `toAuth` extended by the configured headers.
pub struct ComposedOAuthAuth {
    inner: Arc<dyn OAuthAuth>,
    provider_id: String,
    raw_headers: Option<HeaderRecord>,
    auth_header: bool,
}

impl OAuthAuth for ComposedOAuthAuth {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn is_subscription(&self) -> bool {
        self.inner.is_subscription()
    }

    fn login_label(&self) -> Option<&str> {
        self.inner.login_label()
    }

    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        self.inner.login(interaction)
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        self.inner.refresh(credential, options)
    }

    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            let auth = self.inner.to_auth(credential.clone()).await?;
            // Upstream `credential.env` (the OAuth credential index
            // signature); the port carries it through the credential
            // `extra` map.
            let env: Option<ConfigEnv> = credential
                .extra
                .get("env")
                .and_then(|value| value.as_object())
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|(name, value)| {
                            value
                                .as_str()
                                .map(|value| (name.clone(), value.to_string()))
                        })
                        .collect()
                });
            let headers = resolve_headers_or_throw(
                self.raw_headers.as_deref(),
                &format!("provider \"{}\"", self.provider_id),
                env.as_ref(),
            )
            .map_err(AuthError::Operation)?;
            with_configured_auth(&auth, headers.as_ref(), self.auth_header)
                .map_err(AuthError::Operation)
        })
    }
}

/// Upstream `composeOAuthAuth`.
fn compose_oauth_auth(
    provider_id: &str,
    base: Option<&Arc<dyn Provider>>,
    config: Option<&ModelsJsonProvider>,
    extension: Option<&ProviderConfigInput>,
) -> Option<Arc<dyn OAuthAuth>> {
    let oauth: Arc<dyn OAuthAuth> = match extension.and_then(|extension| extension.oauth.clone()) {
        Some(extension_oauth) => Arc::new(ExtensionOAuthAuth {
            config: extension_oauth,
        }),
        None => base.and_then(|base| base.auth().oauth.clone())?,
    };
    let raw_headers = configured_headers(config, extension);
    let auth_header = extension
        .and_then(|extension| extension.auth_header)
        .or(config.and_then(|config| config.auth_header))
        .unwrap_or(false);
    Some(Arc::new(ComposedOAuthAuth {
        inner: oauth,
        provider_id: provider_id.to_string(),
        raw_headers,
        auth_header,
    }))
}

/// Upstream `rawModelHeaders`: model-override headers, then the model
/// definition's, then the extension entry's (later spreads win).
fn raw_model_headers(
    model: &Model,
    config: Option<&ModelsJsonProvider>,
    extension: Option<&ProviderConfigInput>,
) -> Option<HeaderRecord> {
    let override_headers = config
        .and_then(|config| config.model_overrides.as_ref())
        .and_then(|overrides| overrides.iter().find(|(id, _)| *id == model.id))
        .and_then(|(_, override_value)| override_value.headers.clone());
    let definition_headers = config
        .and_then(|config| config.models.as_ref())
        .and_then(|models| models.iter().find(|entry| entry.id == model.id))
        .and_then(|entry| entry.headers.clone());
    let extension_headers = extension
        .and_then(|extension| extension.models.as_ref())
        .and_then(|models| models.iter().find(|entry| entry.id == model.id))
        .and_then(|entry| entry.headers.clone());
    let headers = spread_records(
        override_headers.as_ref(),
        spread_records(definition_headers.as_ref(), extension_headers.as_ref()).as_ref(),
    );
    headers.filter(|headers| !headers.is_empty())
}

// ---------------------------------------------------------------------------
// The composed provider
// ---------------------------------------------------------------------------

/// Upstream `validateExtensionProvider` (registration-time validation; the
/// error texts are byte-pinned).
pub fn validate_extension_provider(
    provider_id: &str,
    base: Option<&Arc<dyn Provider>>,
    models_config: Option<&ModelsJsonProvider>,
    extension: &ProviderConfigInput,
) -> Result<(), String> {
    if extension.stream_simple.is_some() && extension.api.is_none() {
        return Err(format!(
            "Provider {provider_id}: \"api\" is required when registering streamSimple."
        ));
    }
    let base_models = base
        .map(|base| base.get_models())
        .transpose()
        .map_err(|error| error.message)?
        .unwrap_or_default();
    apply_extension(
        provider_id,
        &apply_models_json(provider_id, &base_models, models_config)?,
        Some(extension),
    )
    .map(|_| ())
}

/// Mutable per-provider state captured by the composed provider (upstream:
/// the `composeModelProvider` closure variables).
#[derive(Default)]
struct ComposedProviderState {
    refreshed_extension_models: Option<Vec<ExtensionModelDefinition>>,
    extension_oauth_credential: Option<OAuthCredential>,
}

/// The provider [`compose_model_provider`] builds (upstream's `provider`
/// object literal).
pub struct ComposedProvider {
    provider_id: String,
    display_name: String,
    base_url: Option<String>,
    base_headers: Option<ProviderHeaders>,
    base: Option<Arc<dyn Provider>>,
    config: Option<ModelsJsonProvider>,
    extension: Option<ProviderConfigInput>,
    state: Arc<Mutex<ComposedProviderState>>,
    auth: ProviderAuth,
}

impl ComposedProvider {
    /// Upstream `currentExtension()`.
    fn current_extension(&self, state: &ComposedProviderState) -> Option<ProviderConfigInput> {
        let extension = self.extension.as_ref()?;
        match &state.refreshed_extension_models {
            Some(refreshed) => {
                let mut effective = extension.clone();
                effective.models = Some(refreshed.clone());
                Some(effective)
            }
            None => Some(extension.clone()),
        }
    }

    /// Upstream `getModels` (the composed layering; structural errors surface
    /// through the [`ModelsError`] channel — upstream `getModels` throws and
    /// the collection treats a throwing implementation as having no models).
    fn composed_models(&self) -> Result<Vec<Model>, String> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let base_models = self
            .base
            .as_ref()
            .map(|base| base.get_models())
            .transpose()
            .map_err(|error| error.message)?
            .unwrap_or_default();
        let mut models = apply_extension(
            &self.provider_id,
            &apply_models_json(&self.provider_id, &base_models, self.config.as_ref())?,
            self.current_extension(&state).as_ref(),
        )?;
        if let (Some(credential), Some(extension)) =
            (&state.extension_oauth_credential, &self.extension)
        {
            if let Some(modify_models) = extension
                .oauth
                .as_ref()
                .and_then(|oauth| oauth.modify_models.clone())
            {
                models = modify_models(models, credential);
            }
        }
        // models.json modelOverrides are the topmost user-config layer; they
        // apply once, after custom-model upserts, extension model
        // replacement, and the legacy OAuth projection.
        if let Some(overrides) = &self
            .config
            .as_ref()
            .and_then(|config| config.model_overrides.as_ref())
        {
            for model in &mut models {
                if let Some((_, override_value)) = overrides.iter().find(|(id, _)| *id == model.id)
                {
                    *model = apply_model_override(model.clone(), override_value);
                }
            }
        }
        Ok(models)
    }

    /// Whether the composed provider carries any refresh capability
    /// (upstream `refreshModels:` presence).
    fn has_refresh_models(&self) -> bool {
        let base_dynamic = self.base.as_ref().is_some_and(|base| base.is_dynamic());
        let extension_dynamic = self
            .extension
            .as_ref()
            .is_some_and(|extension| extension.refresh_models.is_some());
        let oauth_dynamic = self
            .extension
            .as_ref()
            .and_then(|extension| extension.oauth.as_ref())
            .is_some_and(|oauth| oauth.modify_models.is_some());
        base_dynamic || extension_dynamic || oauth_dynamic
    }
}

/// Validate a refreshed extension model list before publishing it (upstream
/// runs `applyExtension(applyModelsJson(...))` inside the update closure; a
/// validation failure rejects the publication instead of poisoning state).
fn validate_refreshed_models(
    provider_id: &str,
    base: Option<&Arc<dyn Provider>>,
    config: Option<&ModelsJsonProvider>,
    extension: Option<&ProviderConfigInput>,
    refreshed: &[ExtensionModelDefinition],
) -> Result<(), RefreshModelsError> {
    let mut extension_for_validation = extension.cloned();
    if let Some(extension_for_validation) = extension_for_validation.as_mut() {
        extension_for_validation.models = Some(refreshed.to_vec());
    }
    let base_models = base
        .map(|base| base.get_models())
        .transpose()
        .map_err(RefreshModelsError::Failed)?
        .unwrap_or_default();
    apply_extension(
        provider_id,
        &apply_models_json(provider_id, &base_models, config).map_err(|message| {
            RefreshModelsError::Failed(ModelsError::new(ModelsErrorCode::Provider, message))
        })?,
        extension_for_validation.as_ref(),
    )
    .map_err(|message| {
        RefreshModelsError::Failed(ModelsError::new(ModelsErrorCode::Provider, message))
    })?;
    Ok(())
}
impl Provider for ComposedProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    fn headers(&self) -> Option<&ProviderHeaders> {
        self.base_headers.as_ref()
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
        self.composed_models()
            .map_err(|message| ModelsError::new(ModelsErrorCode::Provider, message))
    }

    fn is_dynamic(&self) -> bool {
        self.has_refresh_models()
    }

    /// Upstream `composed.refreshModels`: base refresh, extension refresh,
    /// then the validated publication of the refreshed list plus the OAuth
    /// credential used for it.
    fn refresh_models(
        &self,
        context: RefreshModelsContext,
    ) -> Option<BoxFuture<'static, Result<(), RefreshModelsError>>> {
        if !self.has_refresh_models() {
            return None;
        }
        // The refresh body works on an owned snapshot of the composition
        // inputs (re-registration replaces the whole provider and supersedes
        // any in-flight refresh like upstream's setProvider supersede).
        let base = self.base.clone();
        let base_has_refresh = self
            .base
            .as_ref()
            .is_some_and(|base| base.refresh_models(context.clone()).is_some());
        let extension = self.extension.clone();
        let config = self.config.clone();
        let provider_id = self.provider_id.clone();
        let state = Arc::clone(&self.state);
        Some(Box::pin(async move {
            if let (Some(base), true) = (&base, base_has_refresh) {
                if let Some(base_refresh) = base.refresh_models(context.clone()) {
                    base_refresh.await?;
                }
            }
            let mut refreshed: Option<Vec<ExtensionModelDefinition>> = None;
            if let Some(extension) = &extension {
                if let Some(extension_refresh) = &extension.refresh_models {
                    refreshed = Some(extension_refresh(context.clone()).await.map_err(
                        |error| match error {
                            AuthError::Models(models_error) => {
                                RefreshModelsError::Failed(models_error)
                            }
                            AuthError::Cancelled => RefreshModelsError::Cancelled,
                            other => RefreshModelsError::Failed(ModelsError::new(
                                ModelsErrorCode::Auth,
                                other.to_string(),
                            )),
                        },
                    )?);
                }
            }
            if context.signal.is_cancelled() {
                return Ok(());
            }
            let oauth_credential = match &context.credential {
                Some(Credential::OAuth(oauth_credential)) => Some(oauth_credential.clone()),
                _ => None,
            };
            // Validate before publishing the new synchronous list.
            if let Some(refreshed) = &refreshed {
                validate_refreshed_models(
                    &provider_id,
                    base.as_ref(),
                    config.as_ref(),
                    extension.as_ref(),
                    refreshed,
                )?;
            }
            let applied = context
                .publish(ModelsPublication {
                    persist: None,
                    update: Some(Box::new(move || {
                        let mut state = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if refreshed.is_some() {
                            state.refreshed_extension_models = refreshed;
                        }
                        state.extension_oauth_credential = oauth_credential;
                    })),
                })
                .await?;
            let _ = applied;
            Ok(())
        }))
    }

    fn filter_models(
        &self,
        models: &[Model],
        credential: Option<&Credential>,
    ) -> Option<Vec<Model>> {
        self.base
            .as_ref()
            .and_then(|base| base.filter_models(models, credential))
    }

    fn api_for(&self, model: &Model) -> Option<Arc<dyn crate::ai::ApiImpl>> {
        // Extension streamSimple override (both upstream entry points route
        // through it when `model.api === extension.api`).
        if let Some(extension) = &self.extension {
            if let (Some(stream_simple), Some(api)) = (&extension.stream_simple, &extension.api) {
                if model.api == *api {
                    return Some(Arc::new(ExtensionApi {
                        stream_simple: Arc::clone(stream_simple),
                    }));
                }
            }
        }
        if let Some(base) = &self.base {
            let supports_base_api = base
                .get_models()
                .unwrap_or_default()
                .iter()
                .any(|entry| entry.api == model.api);
            if supports_base_api {
                return base.api_for(model);
            }
        }
        get_api_provider(&model.api)
    }
}

/// The extension `streamSimple` override behind the [`crate::ai::ApiImpl`]
/// surface (upstream `streamWith`'s first branch).
struct ExtensionApi {
    stream_simple: ExtensionStreamSimpleFn,
}

impl crate::ai::ApiImpl for ExtensionApi {
    fn stream(
        &self,
        _cfg: &crate::ai::ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &crate::ai::types::options::StreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream hands the override the SimpleStreamOptions view of the
        // request options (`options as SimpleStreamOptions`).
        (self.stream_simple)(
            model.clone(),
            ctx.clone(),
            SimpleStreamOptions {
                stream: options.clone(),
                ..SimpleStreamOptions::default()
            },
        )
    }

    fn stream_simple(
        &self,
        _cfg: &crate::ai::ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        (self.stream_simple)(model.clone(), ctx.clone(), options.clone())
    }
}

/// Upstream `getApiProvider` (`pi-ai/compat`): the global wire-API registry
/// over the APIs pi ships adapters for. Upstream also registers legacy api
/// aliases (`legacy-api-aliases.ts`); models carrying those legacy names are
/// not part of this slice's surface and are disclosed.
pub fn get_api_provider(api: &str) -> Option<Arc<dyn crate::ai::ApiImpl>> {
    let implementation: Arc<dyn crate::ai::ApiImpl> = match api {
        "anthropic-messages" => Arc::new(AnthropicMessages),
        "azure-openai-responses" => Arc::new(AzureOpenAiResponses),
        "bedrock-converse-stream" => Arc::new(BedrockConverseStream),
        "google-generative-ai" => Arc::new(GoogleGenerativeAi),
        "google-vertex" => Arc::new(GoogleVertex),
        "mistral-conversations" => Arc::new(MistralConversations),
        "openai-codex-responses" => Arc::new(OpenAiCodexResponses),
        "openai-completions" => Arc::new(OpenAiCompletions),
        "openai-responses" => Arc::new(OpenAiResponses),
        "pi-messages" => Arc::new(PiMessages),
        _ => return None,
    };
    Some(implementation)
}

/// Upstream `composeModelProvider` — compose built-in, models.json, and
/// extension layers without reading credentials.
///
/// The `config` argument is upstream's `modelConfig.getProvider(providerId)`
/// result (`None` when the provider is absent from models.json — the only
/// thing the composer reads from the `ModelConfig` snapshot).
pub fn compose_model_provider(
    provider_id: &str,
    base: Option<Arc<dyn Provider>>,
    config: Option<ModelsJsonProvider>,
    extension: Option<ProviderConfigInput>,
) -> Result<Arc<ComposedProvider>, String> {
    // Validate eagerly so registration/reload reports structural errors
    // immediately (upstream calls getModels() once up front).
    {
        let base_models = base
            .as_ref()
            .map(|base| base.get_models())
            .transpose()
            .map_err(|error| error.message)?
            .unwrap_or_default();
        apply_extension(
            provider_id,
            &apply_models_json(provider_id, &base_models, config.as_ref())?,
            extension.as_ref(),
        )?;
    }

    let api_key = compose_api_key_auth(
        provider_id,
        base.as_ref(),
        config.as_ref(),
        extension.as_ref(),
    );
    let oauth = compose_oauth_auth(
        provider_id,
        base.as_ref(),
        config.as_ref(),
        extension.as_ref(),
    );
    if api_key.is_none() && oauth.is_none() {
        // Unreachable upstream (see the module docs); kept verbatim.
        return Err(format!(
            "Provider {provider_id}: no authentication method configured."
        ));
    }

    let display_name = extension
        .as_ref()
        .and_then(|extension| extension.name.clone())
        .or_else(|| config.as_ref().and_then(|config| config.name.clone()))
        .or_else(|| base.as_ref().map(|base| base.name().to_string()))
        .or_else(|| {
            extension
                .as_ref()
                .and_then(|extension| extension.oauth.as_ref())
                .map(|oauth| oauth.name.clone())
        })
        .unwrap_or_else(|| provider_id.to_string());
    let base_url = extension
        .as_ref()
        .and_then(|extension| extension.base_url.clone())
        .or_else(|| config.as_ref().and_then(|config| config.base_url.clone()))
        .or_else(|| {
            base.as_ref()
                .and_then(|base| base.base_url().map(str::to_string))
        });
    let base_headers = base.as_ref().and_then(|base| base.headers().cloned());
    let auth = ProviderAuth { api_key, oauth };

    Ok(Arc::new(ComposedProvider {
        provider_id: provider_id.to_string(),
        display_name,
        base_url,
        base_headers,
        base,
        config,
        extension,
        state: Arc::new(Mutex::new(ComposedProviderState::default())),
        auth,
    }))
}

// ---------------------------------------------------------------------------
// Header/status helpers
// ---------------------------------------------------------------------------

/// Upstream `resolveConfiguredModelHeaders`.
pub fn resolve_configured_model_headers(
    model: &Model,
    config: Option<&ModelsJsonProvider>,
    extension: Option<&ProviderConfigInput>,
    env: Option<&ConfigEnv>,
) -> Result<Option<HeaderRecord>, String> {
    resolve_headers_or_throw(
        raw_model_headers(model, config, extension).as_deref(),
        &format!("model \"{}/{}\"", model.provider, model.id),
        env,
    )
}

/// Upstream `resolveCompatibilityRequestConfig`.
pub fn resolve_compatibility_request_config(
    model: &Model,
    config: Option<&ModelsJsonProvider>,
    extension: Option<&ProviderConfigInput>,
) -> Result<CompatibilityRequestConfig, String> {
    let merged = spread_records(
        configured_headers(config, extension).as_ref(),
        raw_model_headers(model, config, extension).as_ref(),
    );
    let configured = resolve_headers_or_throw(
        merged.as_deref(),
        &format!("model \"{}/{}\"", model.provider, model.id),
        None,
    )?;
    let headers = if model.headers.is_some() || configured.is_some() {
        let mut merged: ProviderHeaders = model.headers.clone().unwrap_or_default();
        for (key, value) in configured.into_iter().flatten() {
            merged.insert(key, Some(value));
        }
        Some(merged)
    } else {
        None
    };
    Ok(CompatibilityRequestConfig {
        headers,
        auth_header: extension
            .and_then(|extension| extension.auth_header)
            .or(config.and_then(|config| config.auth_header))
            .unwrap_or(false),
    })
}

/// Upstream `configuredRequestAuthStatus`.
pub fn configured_request_auth_status(
    config: Option<&ModelsJsonProvider>,
    extension: Option<&ProviderConfigInput>,
) -> Option<AuthStatus> {
    let value = configured_api_key(config, extension)?;
    if is_command_config_value(value) {
        return Some(AuthStatus::configured(AuthStatusSource::ModelsJsonCommand));
    }
    let names = get_config_value_env_var_names(value);
    if !names.is_empty() {
        return Some(if is_config_value_configured(value, None) {
            AuthStatus {
                configured: true,
                source: Some(AuthStatusSource::Environment),
                label: Some(names.join(", ")),
            }
        } else {
            AuthStatus {
                configured: false,
                source: None,
                label: None,
            }
        });
    }
    Some(AuthStatus::configured(
        if extension
            .and_then(|extension| extension.api_key.as_deref())
            .is_some()
        {
            AuthStatusSource::Fallback
        } else {
            AuthStatusSource::ModelsJsonKey
        },
    ))
}

#[cfg(test)]
#[path = "provider_composer_tests.rs"]
mod tests;
