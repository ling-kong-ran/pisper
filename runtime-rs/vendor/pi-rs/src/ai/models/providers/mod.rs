//! The built-in provider registry, ported from upstream
//! `packages/ai/src/providers/all.ts` plus the 40 per-provider factory files
//! (`providers/*.ts`). Every factory is upstream's `createProvider` call with
//! the provider's id/name/baseUrl, its auth (the [`env_api_key_auth`] default
//! or a custom `ApiKeyAuth`/`lazyOAuth` from M2d), its embedded generated
//! catalog (the port of `Object.values(X_MODELS)`, served by
//! [`embedded_provider_catalog`]), and its API implementation handles
//! ([`ApiImpls`], mapped like upstream's per-API `ProviderStreams` choices).
//!
//! Special providers live in sibling modules:
//! - [`anthropic`] — custom resolve (stored key, then `ANTHROPIC_AUTH_TOKEN`
//!   as `Authorization: Bearer`, then `ANTHROPIC_OAUTH_TOKEN`/
//!   `ANTHROPIC_API_KEY`) plus the Claude Pro/Max OAuth flow.
//! - [`amazon_bedrock`] — bearer token / AWS profile / ambient credential
//!   chain (`auth: {}` results; the Bedrock API impl reads AWS env itself).
//! - [`google_vertex`] — explicit API key or Application Default Credentials
//!   plus project/location env.
//! - [`cloudflare`] — account/gateway env resolution and the stream wrapper
//!   that materializes `{CLOUDFLARE_ACCOUNT_ID}`/`{CLOUDFLARE_GATEWAY_ID}`
//!   URL placeholders from the resolved provider env before dispatch.
//! - [`github_copilot`] — OAuth-only model availability filter
//!   (`credential.availableModelIds`).
//! - [`opencode`] — the `x-opencode-session` per-conversation routing header.
//! - [`radius`] — the purely dynamic gateway provider with its own
//!   `refreshModels` (restore, legacy credential import, gateway config
//!   fetch).
//!
//! # Catalog drift (disclosed in Task 1, unchanged here)
//!
//! `kimi-coding` has a factory (upstream `kimiCodingProvider()`) but no
//! embedded shard — the 2026-09-21 snapshot source (models.dev) had no
//! kimi-coding entry, so [`embedded_provider_catalog`] serves it an empty
//! list, and it is the one static provider besides [`radius`] that lists no
//! models until config-declared/dynamic models fill in. Upstream's committed
//! tree is in the same state (its `data/` is generated and holds no
//! kimi-coding.json in this snapshot).

pub mod amazon_bedrock;
pub mod anthropic;
pub mod cloudflare;
pub mod github_copilot;
pub mod google_vertex;
pub mod opencode;
pub mod radius;

pub use amazon_bedrock::amazon_bedrock_provider;
pub use anthropic::anthropic_provider;
pub use cloudflare::{cloudflare_ai_gateway_provider, cloudflare_workers_ai_provider};
pub use github_copilot::github_copilot_provider;
pub use google_vertex::google_vertex_provider;
pub use opencode::{opencode_go_provider, opencode_provider};
pub use radius::{radius_provider, RadiusProviderOptions};

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::ai::api::anthropic::AnthropicMessages;
use crate::ai::api::azure_openai_responses::AzureOpenAiResponses;
use crate::ai::api::google_generative_ai::GoogleGenerativeAi;
use crate::ai::api::mistral::MistralConversations;
use crate::ai::api::openai_codex_responses::OpenAiCodexResponses;
use crate::ai::api::openai_completions::OpenAiCompletions;
use crate::ai::api::openai_responses::OpenAiResponses;
use crate::ai::api::typesafe_system_one::TypeSafeSystemOneApi;
use crate::ai::auth::helpers::{env_api_key_auth, lazy_oauth, OAuthLoader};
use crate::ai::auth::oauth::load::{
    load_kimi_coding_oauth, load_openai_codex_oauth, load_openrouter_oauth, load_xai_oauth,
};
use crate::ai::auth::types::{AuthError, OAuthAuth, ProviderAuth};
use crate::ai::types::Model;

use super::catalog::{catalog_provider_ids, embedded_provider_catalog, model_data_manifest};
use super::provider::{create_provider, ApiImpls, CreateProviderOptions};
use super::{create_models, CreateModelsOptions, Models, Provider};

/// Upstream `builtinProviders()` (all.ts:89-132): every built-in provider,
/// freshly constructed, in the upstream registration order.
pub fn builtin_providers() -> Vec<Arc<dyn Provider>> {
    vec![
        amazon_bedrock_provider(),
        ant_ling_provider(),
        anthropic_provider(),
        azure_openai_responses_provider(),
        baseten_provider(),
        cerebras_provider(),
        cloudflare_ai_gateway_provider(),
        cloudflare_workers_ai_provider(),
        deepseek_provider(),
        fireworks_provider(),
        github_copilot_provider(),
        google_provider(),
        google_vertex_provider(),
        groq_provider(),
        huggingface_provider(),
        kimi_coding_provider(),
        meta_provider(),
        minimax_provider(),
        minimax_cn_provider(),
        mistral_provider(),
        moonshotai_provider(),
        moonshotai_cn_provider(),
        nvidia_provider(),
        openai_provider(),
        openai_codex_provider(),
        opencode_provider(),
        opencode_go_provider(),
        openrouter_provider(),
        qwen_token_plan_provider(),
        qwen_token_plan_cn_provider(),
        qwen_token_plan_individual_provider(),
        radius_provider(RadiusProviderOptions::default()),
        together_provider(),
        typesafe_provider(),
        vercel_ai_gateway_provider(),
        xai_provider(),
        xiaomi_provider(),
        xiaomi_token_plan_ams_provider(),
        xiaomi_token_plan_cn_provider(),
        xiaomi_token_plan_sgp_provider(),
        zai_provider(),
        zai_coding_cn_provider(),
    ]
}

/// Upstream `builtinModels(options?)` (all.ts:135-141): a `Models` collection
/// with every built-in provider registered.
pub fn builtin_models() -> Models {
    builtin_models_with(CreateModelsOptions::default())
}

/// [`builtin_models`] with explicit collection options (upstream's optional
/// `options` argument).
pub fn builtin_models_with(options: CreateModelsOptions) -> Models {
    let mut models = create_models(options);
    for provider in builtin_providers() {
        models.set_provider(provider);
    }
    models
}

/// Upstream `getBuiltinProviders()` (all.ts:69-71): the provider ids present
/// in the generated catalog, sorted. The generated catalog carries an empty
/// kimi-coding shard in this snapshot (see the module docs), so the id is
/// included like upstream's `MODELS` key even though no data file exists.
pub fn builtin_provider_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = catalog_provider_ids().collect();
    if !ids.contains(&"kimi-coding") {
        ids.push("kimi-coding");
        ids.sort_unstable();
    }
    ids
}

/// Upstream `getBuiltinModel(provider, modelId)` (all.ts:61-67): one model
/// from the generated built-in catalog.
pub fn builtin_model(provider: &str, model_id: &str) -> Option<Model> {
    embedded_provider_catalog(provider)
        .into_iter()
        .find(|model| model.id == model_id)
}

/// Upstream `getBuiltinModelDataGeneratedAt()` (all.ts:74-77): the shared
/// generation timestamp as epoch milliseconds, `None` when the manifest
/// timestamp does not parse (upstream `Number.isNaN(Date.parse(...))`).
pub fn builtin_model_data_generated_at() -> Option<i64> {
    parse_iso_utc_ms(&model_data_manifest().generated_at)
}

// ---------------------------------------------------------------------------
// Factory helpers
// ---------------------------------------------------------------------------

/// One [`ApiImpls::Single`] entry for a unit-struct API implementation.
fn single(implementation: impl crate::ai::ApiImpl + 'static) -> ApiImpls {
    ApiImpls::Single(Arc::new(implementation))
}

/// One [`ApiImpls::PerApi`] map from `(api, implementation)` pairs.
fn per_api(entries: &[(&str, Arc<dyn crate::ai::ApiImpl>)]) -> ApiImpls {
    ApiImpls::PerApi(
        entries
            .iter()
            .map(|(api, implementation)| ((*api).to_string(), Arc::clone(implementation)))
            .collect(),
    )
}

/// An `Arc<dyn ApiImpl>` for a unit-struct implementation (per-API map
/// entries).
fn arc(implementation: impl crate::ai::ApiImpl + 'static) -> Arc<dyn crate::ai::ApiImpl> {
    Arc::new(implementation)
}

/// The upstream `lazyOAuth({ ... load })` form whose loader is a plain
/// factory function (most flows). The loader runs on first OAuth use.
fn lazy_flow(
    name: &str,
    is_subscription: bool,
    login_label: Option<&str>,
    load: fn() -> Arc<dyn OAuthAuth>,
) -> crate::ai::auth::helpers::LazyOAuth {
    lazy_oauth(
        name.to_string(),
        is_subscription,
        login_label.map(str::to_string),
        Arc::new(move || {
            let flow = load();
            Box::pin(async move { Ok(flow) })
                as BoxFuture<'static, Result<Arc<dyn OAuthAuth>, AuthError>>
        }) as Arc<OAuthLoader>,
    )
}

/// The shared body of the thin factories (upstream: `createProvider({ id,
/// name, baseUrl, auth: { apiKey: envApiKeyAuth(label, vars) }, models:
/// Object.values(X_MODELS), api })`).
fn thin_provider(
    id: &str,
    name: &str,
    base_url: Option<&str>,
    key_label: &str,
    env_vars: &[&str],
    api: ApiImpls,
) -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: id.to_string(),
        name: Some(name.to_string()),
        base_url: base_url.map(str::to_string),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(env_api_key_auth(key_label, env_vars)),
            oauth: None,
        },
        models: embedded_provider_catalog(id)
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
    })
}

/// The thin-with-OAuth shape (upstream openrouter/xai/kimi-coding):
/// [`thin_provider`] plus `auth.oauth`.
fn oauth_thin_provider(
    id: &str,
    name: &str,
    base_url: Option<&str>,
    key_label: &str,
    env_vars: &[&str],
    oauth: crate::ai::auth::helpers::LazyOAuth,
    api: ApiImpls,
) -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: id.to_string(),
        name: Some(name.to_string()),
        base_url: base_url.map(str::to_string),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(env_api_key_auth(key_label, env_vars)),
            oauth: Some(Arc::new(oauth)),
        },
        models: embedded_provider_catalog(id)
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
    })
}

/// A classifier-capable catalog: chat entries plus the classifier entries of
/// the generated shard (upstream `[...Object.values(X_MODELS),
/// ...Object.values(X_CLASSIFIER_MODELS)]`). Image entries join where the
/// upstream factory lists them (openrouter only in this snapshot's factories).
fn catalog_with_classifiers(id: &str) -> Vec<crate::ai::types::AnyModel> {
    embedded_provider_catalog(id)
        .into_iter()
        .map(crate::ai::types::AnyModel::Chat)
        .chain(
            crate::ai::models::catalog::embedded_provider_classifier_catalog(id)
                .into_iter()
                .map(crate::ai::types::AnyModel::Classifier),
        )
        .collect()
}

/// One classifier-implementation map (upstream `classifiers: { ... }`).
fn classifiers(
    entries: &[(
        &str,
        Arc<dyn crate::ai::models::provider::ClassifierApiImpl>,
    )],
) -> crate::ai::models::provider::ClassifiersImpls {
    entries
        .iter()
        .map(|(api, implementation)| ((*api).to_string(), Arc::clone(implementation)))
        .collect()
}

// ---------------------------------------------------------------------------
// The thin factories (upstream one file each, in all.ts order)
// ---------------------------------------------------------------------------

/// Upstream `antLingProvider` (ant-ling.ts).
pub fn ant_ling_provider() -> Arc<dyn Provider> {
    thin_provider(
        "ant-ling",
        "Ant Ling",
        Some("https://api.ant-ling.com/v1"),
        "Ant Ling API key",
        &["ANT_LING_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `azureOpenAIResponsesProvider` (azure-openai-responses.ts): no
/// static baseUrl — the API impl resolves the resource endpoint from env.
pub fn azure_openai_responses_provider() -> Arc<dyn Provider> {
    thin_provider(
        "azure-openai-responses",
        "Azure OpenAI",
        None,
        "Azure OpenAI API key",
        &["AZURE_OPENAI_API_KEY"],
        single(AzureOpenAiResponses),
    )
}

/// Upstream `basetenProvider` (baseten.ts).
pub fn baseten_provider() -> Arc<dyn Provider> {
    thin_provider(
        "baseten",
        "Baseten",
        Some("https://inference.baseten.co/v1"),
        "Baseten API key",
        &["BASETEN_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `cerebrasProvider` (cerebras.ts).
pub fn cerebras_provider() -> Arc<dyn Provider> {
    thin_provider(
        "cerebras",
        "Cerebras",
        Some("https://api.cerebras.ai/v1"),
        "Cerebras API key",
        &["CEREBRAS_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `deepseekProvider` (deepseek.ts).
pub fn deepseek_provider() -> Arc<dyn Provider> {
    thin_provider(
        "deepseek",
        "DeepSeek",
        Some("https://api.deepseek.com"),
        "DeepSeek API key",
        &["DEEPSEEK_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `fireworksProvider` (fireworks.ts): a mixed-API provider —
/// anthropic-messages and openai-completions entries each get their own
/// implementation.
pub fn fireworks_provider() -> Arc<dyn Provider> {
    thin_provider(
        "fireworks",
        "Fireworks",
        Some("https://api.fireworks.ai/inference"),
        "Fireworks API key",
        &["FIREWORKS_API_KEY"],
        per_api(&[
            ("anthropic-messages", arc(AnthropicMessages)),
            ("openai-completions", arc(OpenAiCompletions)),
        ]),
    )
}

/// Upstream `googleProvider` (google.ts).
pub fn google_provider() -> Arc<dyn Provider> {
    thin_provider(
        "google",
        "Google",
        Some("https://generativelanguage.googleapis.com/v1beta"),
        "Gemini API key",
        &["GEMINI_API_KEY"],
        single(GoogleGenerativeAi),
    )
}

/// Upstream `groqProvider` (groq.ts).
pub fn groq_provider() -> Arc<dyn Provider> {
    thin_provider(
        "groq",
        "Groq",
        Some("https://api.groq.com/openai/v1"),
        "Groq API key",
        &["GROQ_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `huggingfaceProvider` (huggingface.ts).
pub fn huggingface_provider() -> Arc<dyn Provider> {
    thin_provider(
        "huggingface",
        "Hugging Face",
        Some("https://router.huggingface.co/v1"),
        "Hugging Face token",
        &["HF_TOKEN"],
        single(OpenAiCompletions),
    )
}

/// Upstream `kimiCodingProvider` (kimi-coding.ts): env key plus the Kimi Code
/// subscription OAuth flow. The generated catalog is empty in this snapshot
/// (module docs); config-declared models cover the provider.
pub fn kimi_coding_provider() -> Arc<dyn Provider> {
    oauth_thin_provider(
        "kimi-coding",
        "Kimi For Coding",
        Some("https://api.kimi.com/coding"),
        "Kimi API key",
        &["KIMI_API_KEY"],
        lazy_flow(
            "Kimi Code (subscription)",
            true,
            Some("Sign in with Kimi Code"),
            load_kimi_coding_oauth,
        ),
        single(AnthropicMessages),
    )
}

/// Upstream `minimaxProvider` (minimax.ts): Anthropic-messages wire on the
/// international MiniMax endpoint.
pub fn minimax_provider() -> Arc<dyn Provider> {
    thin_provider(
        "minimax",
        "MiniMax",
        Some("https://api.minimax.io/anthropic"),
        "MiniMax API key",
        &["MINIMAX_API_KEY"],
        single(AnthropicMessages),
    )
}

/// Upstream `minimaxCnProvider` (minimax-cn.ts).
pub fn minimax_cn_provider() -> Arc<dyn Provider> {
    thin_provider(
        "minimax-cn",
        "MiniMax CN",
        Some("https://api.minimaxi.com/anthropic"),
        "MiniMax CN API key",
        &["MINIMAX_CN_API_KEY"],
        single(AnthropicMessages),
    )
}

/// Upstream `mistralProvider` (mistral.ts).
pub fn mistral_provider() -> Arc<dyn Provider> {
    thin_provider(
        "mistral",
        "Mistral",
        Some("https://api.mistral.ai"),
        "Mistral API key",
        &["MISTRAL_API_KEY"],
        single(MistralConversations),
    )
}

/// Upstream `moonshotaiProvider` (moonshotai.ts).
pub fn moonshotai_provider() -> Arc<dyn Provider> {
    thin_provider(
        "moonshotai",
        "Moonshot AI",
        Some("https://api.moonshot.ai/v1"),
        "Moonshot AI API key",
        &["MOONSHOT_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `moonshotaiCnProvider` (moonshotai-cn.ts): the CN endpoint shares
/// the international `MOONSHOT_API_KEY` env var.
pub fn moonshotai_cn_provider() -> Arc<dyn Provider> {
    thin_provider(
        "moonshotai-cn",
        "Moonshot AI CN",
        Some("https://api.moonshot.cn/v1"),
        "Moonshot AI API key",
        &["MOONSHOT_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `nvidiaProvider` (nvidia.ts).
pub fn nvidia_provider() -> Arc<dyn Provider> {
    thin_provider(
        "nvidia",
        "NVIDIA",
        Some("https://integrate.api.nvidia.com/v1"),
        "NVIDIA API key",
        &["NVIDIA_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `openaiProvider` (openai.ts): env key plus the Sign in with
/// ChatGPT subscription flow (the openai.ts delta). The flow loader is the
/// shared seam; the flow itself lands with the auth/oauth slice, so the
/// lazy hook reports the login unavailable when exercised.
pub fn openai_provider() -> Arc<dyn Provider> {
    oauth_thin_provider(
        "openai",
        "OpenAI",
        Some("https://api.openai.com/v1"),
        "OpenAI API key",
        &["OPENAI_API_KEY"],
        lazy_oauth(
            "OpenAI (ChatGPT subscription)".to_string(),
            true,
            Some("Sign in with ChatGPT".to_string()),
            Arc::new(
                || match crate::ai::auth::oauth::load::load_openai_chatgpt_oauth() {
                    Ok(flow) => {
                        let flow: Arc<dyn crate::ai::auth::types::OAuthAuth> = flow;
                        Box::pin(async move { Ok(flow) })
                            as BoxFuture<
                                'static,
                                Result<Arc<dyn crate::ai::auth::types::OAuthAuth>, AuthError>,
                            >
                    }
                    Err(message) => Box::pin(async move { Err(AuthError::Operation(message)) })
                        as BoxFuture<
                            'static,
                            Result<Arc<dyn crate::ai::auth::types::OAuthAuth>, AuthError>,
                        >,
                },
            ) as Arc<OAuthLoader>,
        ),
        single(OpenAiResponses),
    )
}

/// Upstream `openaiCodexProvider` (openai-codex.ts): OAuth-only — the codex
/// backend has no API-key env var, so `auth.api_key` is absent.
pub fn openai_codex_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "openai-codex".to_string(),
        name: Some("OpenAI Codex (legacy)".to_string()),
        base_url: Some("https://chatgpt.com/backend-api".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: None,
            oauth: Some(Arc::new(lazy_flow(
                "OpenAI (ChatGPT Plus/Pro)",
                true,
                None,
                load_openai_codex_oauth,
            ))),
        },
        models: embedded_provider_catalog("openai-codex")
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        api: single(OpenAiCodexResponses),
    })
}

/// Upstream `openrouterProvider` (openrouter.ts): env key or OAuth, with
/// per-API implementations for both wire formats. Session-affinity routing is
/// the API implementations' compat auto-detection (M2b), not factory logic.
pub fn openrouter_provider() -> Arc<dyn Provider> {
    // Upstream #9948: the chat provider also carries the image models (the
    // deleted providers/openrouter-images.ts factory became the `images`
    // map) and the System One classifier models.
    let models: Vec<crate::ai::types::AnyModel> = embedded_provider_catalog("openrouter")
        .into_iter()
        .map(crate::ai::types::AnyModel::Chat)
        .chain(
            crate::ai::models::catalog::embedded_provider_image_catalog("openrouter")
                .into_iter()
                .map(crate::ai::types::AnyModel::Image),
        )
        .chain(
            crate::ai::models::catalog::embedded_provider_classifier_catalog("openrouter")
                .into_iter()
                .map(crate::ai::types::AnyModel::Classifier),
        )
        .collect();
    let mut images = crate::ai::models::provider::ImagesImpls::new();
    images.insert(
        "openrouter-images".to_string(),
        Arc::new(crate::ai::images::openrouter_images::OpenRouterImagesApi),
    );
    create_provider(CreateProviderOptions {
        id: "openrouter".to_string(),
        name: Some("OpenRouter".to_string()),
        base_url: Some("https://openrouter.ai/api/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(env_api_key_auth(
                "OpenRouter API key",
                &["OPENROUTER_API_KEY"],
            )),
            oauth: Some(Arc::new(lazy_flow(
                "OpenRouter OAuth",
                false,
                Some("Sign in with OpenRouter"),
                load_openrouter_oauth,
            ))),
        },
        models,
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api: per_api(&[
            ("anthropic-messages", arc(AnthropicMessages)),
            ("openai-completions", arc(OpenAiCompletions)),
        ]),
        images,
        classifiers: classifiers(&[("typesafe-system-one", Arc::new(TypeSafeSystemOneApi))]),
    })
}

/// Upstream `qwenTokenPlanProvider` (qwen-token-plan.ts).
pub fn qwen_token_plan_provider() -> Arc<dyn Provider> {
    thin_provider(
        "qwen-token-plan",
        "Qwen Token Plan",
        Some("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"),
        "Qwen Token Plan API key",
        &["QWEN_TOKEN_PLAN_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `qwenTokenPlanCnProvider` (qwen-token-plan-cn.ts).
pub fn qwen_token_plan_cn_provider() -> Arc<dyn Provider> {
    thin_provider(
        "qwen-token-plan-cn",
        "Qwen Token Plan CN",
        Some("https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"),
        "Qwen Token Plan CN API key",
        &["QWEN_TOKEN_PLAN_CN_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `qwenTokenPlanIndividualProvider` (qwen-token-plan-individual.ts):
/// the individual plan shares the international endpoint and its env var.
pub fn qwen_token_plan_individual_provider() -> Arc<dyn Provider> {
    thin_provider(
        "qwen-token-plan-individual",
        "Qwen Token Plan Individual",
        Some("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"),
        "Qwen Token Plan Individual API key",
        &["QWEN_TOKEN_PLAN_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `togetherProvider` (together.ts).
pub fn together_provider() -> Arc<dyn Provider> {
    thin_provider(
        "together",
        "Together",
        Some("https://api.together.ai/v1"),
        "Together API key",
        &["TOGETHER_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `vercelAIGatewayProvider` (vercel-ai-gateway.ts).
/// Upstream `vercelAIGatewayProvider` (vercel-ai-gateway.ts, the #9948
/// delta): AI Gateway serves TypeSafe's System One protocol, so the
/// classifier models join the chat catalog and the implementation map.
pub fn vercel_ai_gateway_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "vercel-ai-gateway".to_string(),
        name: Some("Vercel AI Gateway".to_string()),
        base_url: Some("https://ai-gateway.vercel.sh".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(env_api_key_auth(
                "Vercel AI Gateway API key",
                &["AI_GATEWAY_API_KEY"],
            )),
            oauth: None,
        },
        models: catalog_with_classifiers("vercel-ai-gateway"),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api: single(AnthropicMessages),
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: classifiers(&[("typesafe-system-one", Arc::new(TypeSafeSystemOneApi))]),
    })
}

/// Upstream `xaiProvider` (xai.ts): env key plus the Grok/X subscription
/// OAuth flow.
pub fn xai_provider() -> Arc<dyn Provider> {
    oauth_thin_provider(
        "xai",
        "xAI",
        Some("https://api.x.ai/v1"),
        "xAI API key",
        &["XAI_API_KEY"],
        lazy_flow(
            "xAI (Grok/X subscription)",
            true,
            Some("Sign in with SuperGrok or X Premium"),
            load_xai_oauth,
        ),
        single(OpenAiResponses),
    )
}

/// Upstream `xiaomiProvider` (xiaomi.ts).
pub fn xiaomi_provider() -> Arc<dyn Provider> {
    thin_provider(
        "xiaomi",
        "Xiaomi",
        Some("https://api.xiaomimimo.com/v1"),
        "Xiaomi API key",
        &["XIAOMI_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `xiaomiTokenPlanAmsProvider` (xiaomi-token-plan-ams.ts).
pub fn xiaomi_token_plan_ams_provider() -> Arc<dyn Provider> {
    thin_provider(
        "xiaomi-token-plan-ams",
        "Xiaomi Token Plan AMS",
        Some("https://token-plan-ams.xiaomimimo.com/v1"),
        "Xiaomi Token Plan AMS API key",
        &["XIAOMI_TOKEN_PLAN_AMS_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `xiaomiTokenPlanCnProvider` (xiaomi-token-plan-cn.ts).
pub fn xiaomi_token_plan_cn_provider() -> Arc<dyn Provider> {
    thin_provider(
        "xiaomi-token-plan-cn",
        "Xiaomi Token Plan CN",
        Some("https://token-plan-cn.xiaomimimo.com/v1"),
        "Xiaomi Token Plan CN API key",
        &["XIAOMI_TOKEN_PLAN_CN_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `xiaomiTokenPlanSgpProvider` (xiaomi-token-plan-sgp.ts).
pub fn xiaomi_token_plan_sgp_provider() -> Arc<dyn Provider> {
    thin_provider(
        "xiaomi-token-plan-sgp",
        "Xiaomi Token Plan SGP",
        Some("https://token-plan-sgp.xiaomimimo.com/v1"),
        "Xiaomi Token Plan SGP API key",
        &["XIAOMI_TOKEN_PLAN_SGP_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `zaiProvider` (zai.ts).
pub fn zai_provider() -> Arc<dyn Provider> {
    thin_provider(
        "zai",
        "Z.AI",
        Some("https://api.z.ai/api/coding/paas/v4"),
        "Z.AI API key",
        &["ZAI_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `zaiCodingCnProvider` (zai-coding-cn.ts): the CN coding plan on
/// the bigmodel endpoint with its own env var.
pub fn zai_coding_cn_provider() -> Arc<dyn Provider> {
    thin_provider(
        "zai-coding-cn",
        "Z.AI Coding CN",
        Some("https://open.bigmodel.cn/api/coding/paas/v4"),
        "Z.AI Coding CN API key",
        &["ZAI_CODING_CN_API_KEY"],
        single(OpenAiCompletions),
    )
}

/// Upstream `metaProvider` (meta.ts, the Meta Muse delta): the Meta Model
/// API with the Muse subscription OAuth flow (the flow lands with the
/// auth/oauth slice; the lazy hook reports it unavailable meanwhile).
pub fn meta_provider() -> Arc<dyn Provider> {
    oauth_thin_provider(
        "meta",
        "Meta",
        Some("https://api.meta.ai/v1"),
        "Meta Model API key",
        &["META_API_KEY"],
        lazy_oauth(
            "Meta (Muse subscription)".to_string(),
            true,
            Some("Sign in with Meta".to_string()),
            Arc::new(|| match crate::ai::auth::oauth::load::load_meta_oauth() {
                Ok(flow) => {
                    let flow: Arc<dyn crate::ai::auth::types::OAuthAuth> = flow;
                    Box::pin(async move { Ok(flow) })
                        as BoxFuture<
                            'static,
                            Result<Arc<dyn crate::ai::auth::types::OAuthAuth>, AuthError>,
                        >
                }
                Err(message) => Box::pin(async move { Err(AuthError::Operation(message)) })
                    as BoxFuture<
                        'static,
                        Result<Arc<dyn crate::ai::auth::types::OAuthAuth>, AuthError>,
                    >,
            }) as Arc<OAuthLoader>,
        ),
        single(OpenAiResponses),
    )
}

/// Upstream `typesafeProvider` (typesafe.ts, the #9948 delta):
/// classifier-only provider — no chat `api`, just the System One
/// classifier implementation over its generated catalog.
pub fn typesafe_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "typesafe".to_string(),
        name: Some("TypeSafe".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(env_api_key_auth("TypeSafe API key", &["TYPESAFE_API_KEY"])),
            oauth: None,
        },
        models: crate::ai::models::catalog::embedded_provider_classifier_catalog("typesafe")
            .into_iter()
            .map(crate::ai::types::AnyModel::Classifier)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api: ApiImpls::None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: classifiers(&[("typesafe-system-one", Arc::new(TypeSafeSystemOneApi))]),
    })
}

/// `Date.parse` reduced to the generator's UTC ISO-8601 shape
/// (`YYYY-MM-DDTHH:MM:SS[.fff]Z`, the `Date.toISOString()` form): epoch
/// milliseconds, `None` on anything else. Same narrowing as the embedded
/// catalog's `generatedAt` validation, including its documented divergence:
/// day values `01`-`31` are accepted without month-length checks, where
/// upstream `Date.parse` returns `NaN` for impossible dates.
pub(crate) fn parse_iso_utc_ms(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    let fraction_ms = match bytes.len() {
        // `...SSZ`, no fraction.
        20 if bytes[19] == b'Z' => 0,
        // `...SS.fZ` — at least one fraction digit before the trailing `Z`.
        len if len > 21 && bytes[19] == b'.' && bytes[len - 1] == b'Z' => {
            let digits = &bytes[20..len - 1];
            if !digits.iter().all(u8::is_ascii_digit) {
                return None;
            }
            // First three digits are the milliseconds (shorter fractions are
            // zero-padded; longer ones truncate, matching the V8 parser).
            let mut ms = 0u32;
            for i in 0..3 {
                ms *= 10;
                if let Some(digit) = digits.get(i) {
                    ms += u32::from(digit - b'0');
                }
            }
            ms as i64
        }
        _ => return None,
    };
    let digits = |slice: &[u8]| slice.iter().all(u8::is_ascii_digit);
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || !(digits(&bytes[0..4])
            && digits(&bytes[5..7])
            && digits(&bytes[8..10])
            && digits(&bytes[11..13])
            && digits(&bytes[14..16])
            && digits(&bytes[17..19]))
    {
        return None;
    }
    let num = |slice: &[u8]| -> Option<u32> {
        slice.iter().try_fold(0u32, |acc, byte| {
            acc.checked_mul(10)?.checked_add(u32::from(byte - b'0'))
        })
    };
    let year = num(&bytes[0..4])? as i64;
    let month = num(&bytes[5..7])? as i64;
    let day = num(&bytes[8..10])? as i64;
    let hour = num(&bytes[11..13])? as i64;
    let minute = num(&bytes[14..16])? as i64;
    let second = num(&bytes[17..19])? as i64;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    // Days-from-civil (Hinnant) for the UTC date part.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1_000 + fraction_ms)
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::models::get_supported_thinking_levels;
    use crate::ai::types::primitives::ModelCost;
    use std::collections::BTreeMap;

    fn provider_ids(providers: &[Arc<dyn Provider>]) -> Vec<&str> {
        providers.iter().map(|provider| provider.id()).collect()
    }

    /// Upstream providers.test.ts "builtinModels registers every builtin
    /// provider with models" (providers.test.ts:37-56), with the snapshot's
    /// documented kimi-coding drift.
    #[test]
    fn builtin_models_registers_every_builtin_provider_with_models() {
        let models = builtin_models();
        let providers = models.get_providers();
        assert_eq!(providers.len(), builtin_providers().len());
        assert_eq!(provider_ids(&providers)[2], "anthropic");

        let anthropic = models.get_model("anthropic", "claude-haiku-4-5").unwrap();
        assert_eq!(anthropic.api, "anthropic-messages");

        let all = models.get_models(None);
        assert!(all.len() > 500, "{} models total", all.len());

        // Static providers list models immediately; radius is purely dynamic,
        // kimi-coding/typesafe/meta have no generated chat shard in this
        // snapshot (typesafe is classifier-only), so their chat lists are
        // empty.
        for provider in &providers {
            let list = models.get_models(Some(provider.id()));
            if matches!(
                provider.id(),
                "radius" | "kimi-coding" | "typesafe" | "meta"
            ) {
                assert!(
                    list.is_empty(),
                    "{} should list no chat models",
                    provider.id()
                );
            } else {
                assert!(!list.is_empty(), "{} lists no models", provider.id());
            }
            assert!(
                list.iter().all(|model| model.provider == provider.id()),
                "{} leaks foreign models",
                provider.id()
            );
        }
    }

    /// The 40 built-in factories in the upstream all.ts registration order.
    #[test]
    fn builtin_providers_lists_all_factories_in_upstream_order() {
        let expected: &[&str] = &[
            "amazon-bedrock",
            "ant-ling",
            "anthropic",
            "azure-openai-responses",
            "baseten",
            "cerebras",
            "cloudflare-ai-gateway",
            "cloudflare-workers-ai",
            "deepseek",
            "fireworks",
            "github-copilot",
            "google",
            "google-vertex",
            "groq",
            "huggingface",
            "kimi-coding",
            "meta",
            "minimax",
            "minimax-cn",
            "mistral",
            "moonshotai",
            "moonshotai-cn",
            "nvidia",
            "openai",
            "openai-codex",
            "opencode",
            "opencode-go",
            "openrouter",
            "qwen-token-plan",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
            "radius",
            "together",
            "typesafe",
            "vercel-ai-gateway",
            "xai",
            "xiaomi",
            "xiaomi-token-plan-ams",
            "xiaomi-token-plan-cn",
            "xiaomi-token-plan-sgp",
            "zai",
            "zai-coding-cn",
        ];
        assert_eq!(provider_ids(&builtin_providers()), expected, "all.ts order");
    }

    /// Every built-in catalog model dispatches: the provider's API map covers
    /// each model's `api` (upstream: the typed `Provider<TApi>` contract).
    #[test]
    fn every_builtin_model_has_an_api_implementation() {
        for provider in builtin_providers() {
            let models = provider.get_models().unwrap();
            for model in &models {
                assert!(
                    provider.api_for(model).is_some(),
                    "{}/{} has no API implementation",
                    provider.id(),
                    model.id
                );
            }
        }
    }

    /// Catalog counts match the manifest's per-provider model structure
    /// (upstream: the generated shards are the manifest's source of truth).
    #[test]
    fn catalog_counts_match_the_manifest_structure() {
        let structure = crate::ai::models::model_data_structure();
        for (provider_id, models) in structure {
            let catalog = embedded_provider_catalog(provider_id);
            assert_eq!(
                catalog.len(),
                models.len(),
                "{provider_id} catalog count drifts from the manifest"
            );
            for model in &catalog {
                assert_eq!(
                    models.get(&model.id).map(String::as_str),
                    Some(model.api.as_str()),
                    "{provider_id}/{} api mismatch",
                    model.id
                );
            }
        }
        // Task 1 oracle pins.
        assert_eq!(embedded_provider_catalog("xai").len(), 3);
        assert_eq!(embedded_provider_catalog("openrouter").len(), 380);
    }

    /// Upstream providers.test.ts "stores native constrained-sampling
    /// capabilities in model metadata" (providers.test.ts:58-67). The port's
    /// `Model.compat` is a raw JSON value, so the pins read the upstream
    /// camelCase keys.
    #[test]
    fn stores_native_constrained_sampling_capabilities_in_model_metadata() {
        let gpt4o = builtin_model("openai", "gpt-4o").unwrap();
        assert_eq!(
            gpt4o.compat.as_ref().unwrap().get("supportsStrictMode"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            gpt4o
                .compat
                .as_ref()
                .unwrap()
                .get("supportsOpenAIGrammarTools"),
            None
        );
        let gpt54 = builtin_model("openai", "gpt-5.4").unwrap().compat.unwrap();
        assert_eq!(
            gpt54.get("supportsStrictMode"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            gpt54.get("supportsOpenAIGrammarTools"),
            Some(&serde_json::json!(true))
        );
        let haiku = builtin_model("anthropic", "claude-haiku-4-5")
            .unwrap()
            .compat
            .unwrap();
        assert_eq!(
            haiku.get("supportsStrictTools"),
            Some(&serde_json::json!(true))
        );
    }

    /// Upstream providers.test.ts "enables mid-conversation system messages
    /// only for verified models" (providers.test.ts:92-149); the
    /// `google/gemini-2.5-pro` row is dropped (absent from this snapshot).
    #[test]
    fn enables_mid_conversation_system_messages_only_for_verified_models() {
        let models = builtin_models();
        let supported = [
            ("moonshotai", "kimi-k2.6"),
            ("moonshotai", "kimi-k2.7-code"),
            ("moonshotai", "kimi-k2.7-code-highspeed"),
            ("moonshotai", "kimi-k3"),
            ("moonshotai-cn", "kimi-k2.6"),
            ("moonshotai-cn", "kimi-k2.7-code"),
            ("moonshotai-cn", "kimi-k2.7-code-highspeed"),
            ("moonshotai-cn", "kimi-k3"),
            ("fireworks", "accounts/fireworks/models/kimi-k3"),
            ("fireworks", "accounts/fireworks/routers/kimi-k3-fast"),
            ("openai", "gpt-5.4"),
            ("openai", "gpt-5.5"),
            ("openai", "gpt-6-astra"),
            ("openai-codex", "gpt-5.5"),
            ("anthropic", "claude-opus-5"),
            ("opencode", "gpt-5.4"),
            ("opencode", "gpt-5.6-terra"),
            ("opencode-go", "gpt-5.6-luna"),
            ("opencode", "claude-opus-4-8"),
            ("opencode", "claude-opus-5"),
            ("opencode", "kimi-k3"),
            ("opencode-go", "kimi-k3"),
            ("github-copilot", "gpt-5.6-terra"),
            ("github-copilot", "claude-opus-5"),
            ("github-copilot", "claude-opus-4.8"),
            ("github-copilot", "kimi-k3"),
            ("deepseek", "deepseek-v4-pro"),
            ("openrouter", "openai/gpt-5.6-terra"),
        ];
        let unsupported = [
            ("fireworks", "accounts/fireworks/models/kimi-k2p6"),
            ("openai", "gpt-4.1"),
            ("openai", "gpt-5.2"),
            ("anthropic", "claude-sonnet-4-5"),
            ("opencode", "gpt-5.2"),
            ("opencode", "claude-sonnet-4-5"),
            ("github-copilot", "claude-sonnet-4.6"),
            ("deepseek", "deepseek-flash"),
            ("openrouter", "anthropic/claude-opus-5"),
            ("openrouter", "moonshotai/kimi-k3"),
            ("openrouter", "openai/gpt-5.6-terra:batch"),
        ];
        for (provider, model_id) in supported {
            let model = models
                .get_model(provider, model_id)
                .unwrap_or_else(|| panic!("{provider}/{model_id} missing"));
            let flag = model
                .compat
                .as_ref()
                .and_then(|compat| compat.get("supportsMidConvoSystemMessages"));
            assert_eq!(
                flag,
                Some(&serde_json::json!(true)),
                "{provider}/{model_id}"
            );
        }
        for (provider, model_id) in unsupported {
            let model = models
                .get_model(provider, model_id)
                .unwrap_or_else(|| panic!("{provider}/{model_id} missing"));
            let flag = model
                .compat
                .as_ref()
                .and_then(|compat| compat.get("supportsMidConvoSystemMessages"));
            assert_ne!(
                flag,
                Some(&serde_json::json!(true)),
                "{provider}/{model_id}"
            );
        }
    }

    /// Upstream providers.test.ts "routes proxied tool changes through
    /// verified transports only" (providers.test.ts:151-191).
    #[test]
    fn routes_proxied_tool_changes_through_verified_transports_only() {
        let models = builtin_models();
        for (provider, model_id) in [
            ("opencode", "gpt-5.6-terra"),
            ("github-copilot", "gpt-5.6-terra"),
        ] {
            let compat = models
                .get_model(provider, model_id)
                .unwrap()
                .compat
                .unwrap();
            assert_eq!(
                compat.get("supportsAdditionalTools"),
                Some(&serde_json::json!(true)),
                "{provider}/{model_id}"
            );
            assert_ne!(
                compat.get("supportsToolSearch"),
                Some(&serde_json::json!(true)),
                "{provider}/{model_id}"
            );
        }
        for provider in ["opencode", "github-copilot"] {
            let compat = models
                .get_model(provider, "claude-opus-5")
                .unwrap()
                .compat
                .unwrap();
            assert_ne!(
                compat.get("supportsMidConvoToolChanges"),
                Some(&serde_json::json!(true)),
                "{provider}/claude-opus-5"
            );
        }
        assert_eq!(
            models
                .get_model("anthropic", "claude-opus-5")
                .unwrap()
                .compat
                .unwrap()
                .get("supportsMidConvoToolChanges"),
            Some(&serde_json::json!(true))
        );
        for provider in ["moonshotai", "moonshotai-cn", "opencode", "opencode-go"] {
            let compat = models
                .get_model(provider, "kimi-k3")
                .unwrap()
                .compat
                .unwrap();
            assert_eq!(
                compat.get("supportsMidConvoToolAdditions"),
                Some(&serde_json::json!(true)),
                "{provider}/kimi-k3"
            );
        }
        for provider in ["moonshotai", "moonshotai-cn"] {
            for model_id in ["kimi-k2.6", "kimi-k2.7-code", "kimi-k2.7-code-highspeed"] {
                let compat = models
                    .get_model(provider, model_id)
                    .unwrap()
                    .compat
                    .unwrap();
                assert_ne!(
                    compat.get("supportsMidConvoToolAdditions"),
                    Some(&serde_json::json!(true)),
                    "{provider}/{model_id}"
                );
            }
        }
        for (provider, model_id) in [
            ("github-copilot", "kimi-k3"),
            ("openrouter", "openai/gpt-5.6-terra"),
        ] {
            let compat = models
                .get_model(provider, model_id)
                .unwrap()
                .compat
                .unwrap();
            assert_ne!(
                compat.get("supportsMidConvoToolAdditions"),
                Some(&serde_json::json!(true)),
                "{provider}/{model_id}"
            );
        }
    }

    /// Upstream providers.test.ts "uses official Kimi K3 pricing for Moonshot
    /// providers" (providers.test.ts:193-203).
    #[test]
    fn uses_official_kimi_k3_pricing_for_moonshot_providers() {
        let models = builtin_models();
        let expected = ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 0.0,
            tiers: None,
        };
        for provider in ["moonshotai", "moonshotai-cn"] {
            assert_eq!(
                models.get_model(provider, "kimi-k3").unwrap().cost,
                expected
            );
        }
    }

    /// Upstream all.ts `getBuiltinModelDataGeneratedAt` parses the manifest
    /// timestamp (epoch pinned against the ECMAScript `Date.parse`).
    #[test]
    fn builtin_model_data_generated_at_parses_the_manifest_timestamp() {
        assert_eq!(builtin_model_data_generated_at(), Some(1_789_956_503_378));
    }

    /// The ISO parser mirrors `Date.parse` on the generator's shape and
    /// rejects garbage.
    #[test]
    fn parse_iso_utc_ms_matches_date_parse_on_the_generator_shape() {
        assert_eq!(parse_iso_utc_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso_utc_ms("2026-09-21T02:08:23.378Z"),
            Some(1_789_956_503_378)
        );
        assert_eq!(
            parse_iso_utc_ms("2026-09-21T02:08:23.3Z"),
            Some(1_789_956_503_300)
        );
        assert_eq!(
            parse_iso_utc_ms("2026-09-21T02:08:23.3785Z"),
            Some(1_789_956_503_378)
        );
        assert_eq!(parse_iso_utc_ms("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_iso_utc_ms("2026-09-21T24:00:00Z"), None);
        assert_eq!(parse_iso_utc_ms("not a date"), None);
        assert_eq!(parse_iso_utc_ms(""), None);
        // A local-time offset form is not the generator's shape.
        assert_eq!(parse_iso_utc_ms("2026-09-21T02:08:23+02:00"), None);
    }

    /// Upstream `getBuiltinProviders` port: catalog ids plus the drifted
    /// kimi-coding key, sorted.
    #[test]
    fn builtin_provider_ids_include_kimi_coding() {
        let ids = builtin_provider_ids();
        assert_eq!(ids.len(), 39);
        assert!(ids.contains(&"kimi-coding"));
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }

    /// The kimi-coding factory survives the missing shard: full metadata,
    /// empty catalog, OAuth advertised (module-docs drift).
    #[test]
    fn kimi_coding_builds_with_an_empty_catalog() {
        let provider = kimi_coding_provider();
        assert_eq!(provider.id(), "kimi-coding");
        assert_eq!(provider.name(), "Kimi For Coding");
        assert_eq!(provider.base_url(), Some("https://api.kimi.com/coding"));
        assert!(provider.get_models().unwrap().is_empty());
        let auth = provider.auth();
        assert_eq!(
            auth.api_key.as_ref().map(|auth| auth.name()),
            Some("Kimi API key")
        );
        let oauth = auth.oauth.as_ref().unwrap();
        assert_eq!(oauth.name(), "Kimi Code (subscription)");
        assert!(oauth.is_subscription());
        assert_eq!(oauth.login_label(), Some("Sign in with Kimi Code"));
    }

    /// Upstream openai-codex.ts: OAuth-only provider — no api-key auth, and
    /// getAuth reports unconfigured without a stored credential.
    #[tokio::test]
    async fn openai_codex_is_oauth_only() {
        let provider = openai_codex_provider();
        assert_eq!(provider.id(), "openai-codex");
        assert_eq!(provider.name(), "OpenAI Codex (legacy)");
        assert_eq!(provider.base_url(), Some("https://chatgpt.com/backend-api"));
        assert!(provider.auth().api_key.is_none());
        assert_eq!(
            provider.auth().oauth.as_ref().map(|oauth| oauth.name()),
            Some("OpenAI (ChatGPT Plus/Pro)")
        );
        assert!(!provider.get_models().unwrap().is_empty());

        let mut models =
            crate::ai::models::create_models(crate::ai::models::CreateModelsOptions::default());
        models.set_provider(provider);
        let model = models.get_models(Some("openai-codex"))[0].clone();
        assert_eq!(
            models.get_auth(&model.provider, None).await.unwrap(),
            None,
            "codex without a stored credential is unconfigured"
        );
    }

    /// The openai-codex factory lists models whose api maps to the codex
    /// implementation (upstream `Provider<"openai-codex-responses">`).
    #[test]
    fn openai_codex_models_use_the_codex_api() {
        let provider = openai_codex_provider();
        for model in provider.get_models().unwrap() {
            assert_eq!(model.api, "openai-codex-responses");
        }
    }

    /// Upstream providers.test.ts "uses models.dev effort levels for Google
    /// thinking models" (providers.test.ts:69-90) through the
    /// `getSupportedThinkingLevels` port (models.ts:924-933).
    #[test]
    fn uses_models_dev_effort_levels_for_google_thinking_models() {
        let supported = |provider: &str, model_id: &str| {
            get_supported_thinking_levels(&builtin_model(provider, model_id).unwrap())
        };
        for provider in ["google", "google-vertex"] {
            assert!(supported(provider, "gemini-3.6-flash").contains(&"minimal"));
            assert_eq!(
                supported(provider, "gemini-3.8-flash"),
                ["low", "medium", "high"]
            );
            assert_eq!(
                supported(provider, "gemini-3.1-pro-preview"),
                ["low", "medium", "high"]
            );
        }
        assert_eq!(
            supported("opencode", "gemini-3.8-flash"),
            ["low", "medium", "high"]
        );
        assert_eq!(supported("google", "gemma-4-31b-it"), ["minimal", "high"]);
    }

    /// models.ts:924-933 edge cases: non-reasoning models report only "off";
    /// xhigh/max need an explicit mapping.
    #[test]
    fn supported_thinking_levels_edge_cases() {
        let mut model = builtin_model("openai", "gpt-4o").unwrap();
        model.reasoning = false;
        assert_eq!(get_supported_thinking_levels(&model), ["off"]);

        model.reasoning = true;
        model.thinking_level_map = None;
        assert_eq!(
            get_supported_thinking_levels(&model),
            ["off", "minimal", "low", "medium", "high"]
        );

        model.thinking_level_map = Some(BTreeMap::from([
            ("off".to_string(), None),
            ("minimal".to_string(), Some("low".to_string())),
            ("xhigh".to_string(), Some("xhigh".to_string())),
        ]));
        assert_eq!(
            get_supported_thinking_levels(&model),
            ["minimal", "low", "medium", "high", "xhigh"]
        );
    }
}
