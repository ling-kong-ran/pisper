//! The transformation core: upstream constants (generate-models.ts:167-475),
//! compat detection (610-848), the apply-pipeline (3025-3037), the override
//! and additive passes (2560-3023), and the provider grouping/serialization
//! (3039-3087) that produces the per-provider files and the api structure the
//! manifest hashes.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value as Json;

use pi_rust::ai::models::catalog::ModelDataStructure;

use crate::json::{js_or, obj, round_cost, JsObj, Jv};
use crate::providers::{
    fetch_ai_gateway_models, fetch_openrouter_models, load_models_dev_data, LoaderState,
};
use crate::reasoning_options::{
    effort_thinking_level_map, parse_reasoning_options, ReasoningOption,
};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Linear membership test over an upstream `Set`/array literal.
pub(crate) fn set_contains(set: &[&str], value: &str) -> bool {
    set.contains(&value)
}

/// An ordered thinking-level map literal (`level_map(&[("off", None), ...])`).
pub(crate) fn level_map(pairs: &[(&str, Option<&str>)]) -> JsObj {
    JsObj::from_pairs(
        pairs
            .iter()
            .map(|(level, value)| match value {
                Some(value) => (*level, Jv::s(*value)),
                None => (*level, Jv::Null),
            })
            .collect(),
    )
}

pub(crate) fn string_value<'a>(model: &'a JsObj, key: &str) -> Option<&'a str> {
    model.get(key).and_then(Jv::as_str)
}

fn compat_obj(model: &JsObj) -> Option<&JsObj> {
    match model.get("compat") {
        Some(Jv::Obj(compat)) => Some(compat),
        _ => None,
    }
}

fn compat_flag(model: &JsObj, key: &str) -> Option<bool> {
    match compat_obj(model).and_then(|compat| compat.get(key)) {
        Some(Jv::Bool(value)) => Some(*value),
        _ => None,
    }
}

/// `model.compat = {...model.compat, ...compat}` for any of the upstream
/// merge helpers.
fn merge_compat(model: &mut JsObj, compat: JsObj) {
    let merged = match compat_obj(model) {
        Some(existing) => {
            let mut merged = existing.clone();
            merged.spread(&compat);
            merged
        }
        None => compat,
    };
    model.set("compat", Jv::Obj(merged));
}

/// Upstream `mergeThinkingLevelMap`: `{...model.thinkingLevelMap, ...map}`.
fn merge_thinking_level_map(model: &mut JsObj, map: JsObj) {
    let merged = match model.get("thinkingLevelMap") {
        Some(Jv::Obj(existing)) => {
            let mut merged = existing.clone();
            merged.spread(&map);
            merged
        }
        _ => map,
    };
    model.set("thinkingLevelMap", Jv::Obj(merged));
}

fn cost_field(model: &JsObj, field: &str) -> f64 {
    model
        .get("cost")
        .and_then(|cost| cost.get(field))
        .and_then(Jv::as_f64)
        .unwrap_or(0.0)
}

fn set_cost_field(model: &mut JsObj, field: &str, value: f64) {
    if let Some(Jv::Obj(cost)) = model.get_mut("cost") {
        cost.set(field, Jv::n(value));
    }
}

fn set_numeric(model: &mut JsObj, key: &str, value: f64) {
    model.set(key, Jv::n(value));
}

// ---------------------------------------------------------------------------
// Constants (generate-models.ts:167-475)
// ---------------------------------------------------------------------------

/// `CLOUDFLARE_*` constants (`../src/api/cloudflare.ts`).
pub(crate) const CLOUDFLARE_WORKERS_AI_BASE_URL: &str =
    "https://api.cloudflare.com/client/v4/accounts/{CLOUDFLARE_ACCOUNT_ID}/ai/v1";
pub(crate) const CLOUDFLARE_AI_GATEWAY_COMPAT_BASE_URL: &str =
    "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/compat";
pub(crate) const CLOUDFLARE_AI_GATEWAY_OPENAI_BASE_URL: &str =
    "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/openai";
pub(crate) const CLOUDFLARE_AI_GATEWAY_ANTHROPIC_BASE_URL: &str =
    "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/anthropic";

pub(crate) const VERTEX_BASE_URL: &str = "https://{location}-aiplatform.googleapis.com";
pub(crate) const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub(crate) const AI_GATEWAY_BASE_URL: &str = "https://ai-gateway.vercel.sh";
pub(crate) const TOGETHER_BASE_URL: &str = "https://api.together.ai/v1";
pub(crate) const KIMI_K3_MAX_TOKENS: f64 = 131_072.0;

pub(crate) const COPILOT_STATIC_HEADERS: &[(&str, &str)] = &[
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];

pub(crate) const NVIDIA_HEADERS: &[(&str, &str)] = &[("NVCF-POLL-SECONDS", "3600")];

pub(crate) const TOGETHER_REASONING_ONLY_MODELS: &[&str] =
    &["deepseek-ai/DeepSeek-R1", "MiniMaxAI/MiniMax-M2.7"];
pub(crate) const TOGETHER_REASONING_EFFORT_MODELS: &[&str] =
    &["openai/gpt-oss-20b", "openai/gpt-oss-120b"];
pub(crate) const TOGETHER_TOGGLE_REASONING_EFFORT_MODELS: &[&str] =
    &["deepseek-ai/DeepSeek-V4-Pro"];

pub(crate) const NVIDIA_NIM_UNSUPPORTED_MODELS: &[&str] = &[
    "abacusai/dracarys-llama-3.1-70b-instruct",
    "bytedance/seed-oss-36b-instruct",
    "deepseek-ai/deepseek-v4-flash",
    "deepseek-ai/deepseek-v4-pro",
    "google/gemma-2-2b-it",
    "google/gemma-3n-e2b-it",
    "google/gemma-3n-e4b-it",
    "google/gemma-4-31b-it",
    "meta/llama-3.2-1b-instruct",
    "meta/llama-4-maverick-17b-128e-instruct",
    "microsoft/phi-4-mini-instruct",
    "minimaxai/minimax-m2.7",
    "mistralai/mistral-nemotron",
    "nvidia/nemotron-mini-4b-instruct",
    "qwen/qwen3-next-80b-a3b-instruct",
    "qwen/qwen3.5-397b-a17b",
    "sarvamai/sarvam-m",
    "upstage/solar-10.7b-instruct",
];

pub(crate) const ZAI_TOOL_STREAM_UNSUPPORTED_MODELS: &[&str] =
    &["glm-4.5", "glm-4.5-air", "glm-4.5-flash", "glm-4.5v"];

pub(crate) const EAGER_TOOL_INPUT_STREAMING_UNSUPPORTED_ANTHROPIC_MODELS: &[&str] = &[
    "github-copilot:claude-haiku-4.5",
    "github-copilot:claude-sonnet-4",
    "github-copilot:claude-sonnet-4.5",
];

/// `ANTHROPIC_ALLOWED_FALLBACK_MODELS`: primary id → fallback ids.
pub(crate) const ANTHROPIC_ALLOWED_FALLBACK_MODELS: &[(&str, &[&str])] = &[
    ("claude-fable-5", &["claude-opus-4-8", "claude-opus-5"]),
    ("claude-opus-5", &["claude-opus-4-8"]),
];

pub(crate) const VERIFIED_ANTHROPIC_MID_CONVO_EFFORT_PROVIDERS: &[&str] =
    &["anthropic", "openrouter"];
/// OpenRouter rejects `configuration_update` system messages on Opus 5
/// ("Mid-conversation reasoning effort (configuration_update) is not supported
/// on anthropic/claude-opus-5-20260723") while accepting them on Fable 5.1.
pub(crate) const MID_CONVO_EFFORT_UNSUPPORTED_ANTHROPIC_MODELS: &[&str] =
    &["openrouter:anthropic/claude-opus-5"];

pub(crate) const OPENAI_GRAMMAR_TOOL_PROVIDERS: &[&str] = &[
    "openai",
    "openai-codex",
    "azure-openai-responses",
    "github-copilot",
    "opencode",
    "cloudflare-ai-gateway",
];
pub(crate) const OPENAI_GRAMMAR_TOOL_APIS: &[&str] = &[
    "openai-responses",
    "azure-openai-responses",
    "openai-codex-responses",
];

pub(crate) const OPENAI_RESPONSES_PROXY_PROVIDERS: &[&str] =
    &["opencode", "opencode-go", "github-copilot"];

pub(crate) const QWEN_TOKEN_PLAN_PROVIDER_IDS: &[&str] = &[
    "qwen-token-plan",
    "qwen-token-plan-cn",
    "qwen-token-plan-individual",
];
/// Retired preview id — models.dev may still list it after GA ships.
pub(crate) const QWEN_TOKEN_PLAN_EXCLUDED_MODEL_IDS: &[&str] = &["qwen3.8-max-preview"];
/// QwenCloud Token Plan Individual text-model allowlist, verified 2026-09-03.
pub(crate) const QWEN_TOKEN_PLAN_INDIVIDUAL_MODEL_IDS: &[&str] = &[
    "deepseek-v4-flash-0731",
    "deepseek-v4-pro",
    "deepseek-v4-pro-0813",
    "glm-5.2",
    "qwen3.6-flash",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.8-flash",
    "qwen3.8-max",
];
pub(crate) const QWEN_TOKEN_PLAN_REASONING_EFFORT_FALLBACK_MODEL_IDS: &[&str] =
    &["glm-5", "glm-5.1"];

/// Kimi Coding alias ids models.dev may expose; normalized to the canonical
/// model id when the canonical entry exists.
pub(crate) const KIMI_ALIASES: &[&str] = &["k2p5", "k2p6", "k2p7"];

/// Kimi Coding is subscription-backed, so models.dev reports zero cost. Use
/// the equivalent Moonshot API rates to estimate the value of subscription
/// usage. Field order: input, output, cacheRead, cacheWrite.
pub(crate) const KIMI_CODING_IMPLIED_COSTS: &[(&str, [(&str, f64); 4])] = &[
    (
        "k3",
        [
            ("input", 3.0),
            ("output", 15.0),
            ("cacheRead", 0.3),
            ("cacheWrite", 0.0),
        ],
    ),
    (
        "kimi-for-coding",
        [
            ("input", 0.95),
            ("output", 4.0),
            ("cacheRead", 0.19),
            ("cacheWrite", 0.0),
        ],
    ),
    (
        "kimi-for-coding-highspeed",
        [
            ("input", 1.9),
            ("output", 8.0),
            ("cacheRead", 0.38),
            ("cacheWrite", 0.0),
        ],
    ),
    (
        "kimi-k2-thinking",
        [
            ("input", 0.6),
            ("output", 2.5),
            ("cacheRead", 0.15),
            ("cacheWrite", 0.0),
        ],
    ),
];

pub(crate) const OPENROUTER_KIMI_K3_MODEL_IDS: &[&str] =
    &["moonshotai/kimi-k3", "~moonshotai/kimi-latest"];

pub(crate) const BEDROCK_INFERENCE_PROFILE_ONLY_MODEL_IDS: &[&str] = &["anthropic.claude-opus-5"];
pub(crate) const MODELS_DEV_OPENAI_UNSUPPORTED_MODEL_IDS: &[&str] = &["gpt-5.6"];
pub(crate) const OPENAI_TOOL_SEARCH_MODEL_IDS: &[&str] = &[
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.4-pro",
    "gpt-5.5",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-6-astra",
];
pub(crate) const OPENAI_CODEX_ADDITIONAL_TOOLS_MODEL_IDS: &[&str] = &[
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-6-astra",
];
pub(crate) const OPENAI_LONG_CONTEXT_INPUT_THRESHOLD: f64 = 272_000.0;
pub(crate) const OPENAI_SHORT_CONTEXT_CAPPED_MODEL_IDS: &[&str] = &[
    "gpt-5.4",
    "gpt-5.5",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-6-astra",
];
pub(crate) const OPENAI_LONG_CONTEXT_PRICING_MODEL_IDS: &[&str] = &[
    "gpt-5.4",
    "gpt-5.4-pro",
    "gpt-5.5",
    "gpt-5.5-pro",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-6-astra",
];

/// OpenAI reduced GPT-5.6 Terra and Luna prices on 2026-07-30. Keep these
/// authoritative values until models.dev and passthrough catalogs catch up.
/// https://developers.openai.com/api/docs/pricing
pub(crate) const OPENAI_GPT_56_STANDARD_COSTS: &[(&str, [(&str, f64); 4])] = &[
    (
        "gpt-5.6-luna",
        [
            ("input", 0.2),
            ("output", 1.2),
            ("cacheRead", 0.02),
            ("cacheWrite", 0.25),
        ],
    ),
    (
        "gpt-5.6-terra",
        [
            ("input", 2.0),
            ("output", 12.0),
            ("cacheRead", 0.2),
            ("cacheWrite", 2.5),
        ],
    ),
];

pub(crate) const OPENAI_RESPONSES_NONE_REASONING_MODELS: &[&str] = &[
    "gpt-5.1",
    "gpt-5.2",
    "gpt-5.3-codex",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.4-nano",
    "gpt-5.5",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
];
pub(crate) const XAI_BUILTIN_EXCLUDED_MODEL_IDS: &[&str] = &[
    "grok-3",
    "grok-3-fast",
    "grok-4.20-0309-non-reasoning",
    "grok-4.20-0309-reasoning",
    "grok-build-0.1",
    "grok-code-fast-1",
];

pub(crate) const OPENCODE_LONG_CACHE_RETENTION_UNSUPPORTED_MODELS: &[&str] = &[
    "opencode:deepseek-v4-flash",
    "opencode:deepseek-v4-pro",
    "opencode:kimi-k2.5",
    "opencode:kimi-k2.6",
    "opencode:minimax-m2.7",
    "opencode-go:kimi-k2.6",
];

/// GitHub's "Models with extended capabilities" table lists these Copilot
/// models as supporting the extended 1 million token context window.
pub(crate) const GITHUB_COPILOT_EXTENDED_CONTEXT_MODELS: &[&str] = &[
    "claude-fable-5",
    "claude-opus-4.6",
    "claude-opus-4.7",
    "claude-opus-4.8",
    "claude-opus-5",
    "claude-sonnet-4.6",
    "claude-sonnet-5",
    "gpt-5.3-codex",
    "gpt-5.4",
    "gpt-5.5",
];

/// Checked manually against the authenticated GitHub Copilot /models endpoint
/// on 2026-06-15. Keep this to narrow corrections over models.dev metadata
/// instead of snapshotting Copilot's catalog.
/// A thinking-level map override: ordered `level -> provider value` pairs.
pub(crate) type LevelOverride<'a> = &'a [(&'a str, Option<&'a str>)];

pub(crate) const GITHUB_COPILOT_THINKING_LEVEL_OVERRIDES: &[(&str, LevelOverride)] = &[
    ("claude-opus-4.7", &[("minimal", Some("low"))]),
    ("claude-opus-4.8", &[("minimal", Some("low"))]),
    ("claude-opus-5", &[("minimal", Some("low"))]),
    (
        "claude-sonnet-4.6",
        &[("minimal", Some("low")), ("max", Some("max"))],
    ),
];

pub(crate) const MINIMAX_DIRECT_SUPPORTED_IDS: &[&str] =
    &["MiniMax-M2.7", "MiniMax-M2.7-highspeed", "MiniMax-M3"];

/// Verified against Fireworks Messages raw_output on 2026-09-10 (#9323). Fall
/// back to verified support when models.dev omits effort metadata; this is
/// not an allowlist.
pub(crate) const FIREWORKS_ADAPTIVE_THINKING_FALLBACK_MODELS: &[&str] = &[
    "accounts/fireworks/models/deepseek-v4-flash-0731",
    "accounts/fireworks/models/deepseek-v4-flash-vision-exp",
    "accounts/fireworks/models/deepseek-v4-pro-0813",
    "accounts/fireworks/models/qwen3p8-max",
    "accounts/fireworks/models/qwen3p8-2p4t-a95b",
];

// Thinking-level map literals.

pub(crate) const TOGETHER_FIXED_REASONING_LEVEL_MAP: &[(&str, Option<&str>)] = &[
    ("off", None),
    ("minimal", None),
    ("low", None),
    ("medium", None),
];
pub(crate) const TOGETHER_REASONING_EFFORT_LEVEL_MAP: &[(&str, Option<&str>)] =
    &[("off", None), ("minimal", None)];
pub(crate) const TOGETHER_DEEPSEEK_V4_THINKING_LEVEL_MAP: &[(&str, Option<&str>)] = &[
    ("minimal", None),
    ("low", None),
    ("medium", None),
    ("high", Some("high")),
    ("xhigh", None),
];
pub(crate) const TOGETHER_TOGGLE_REASONING_LEVEL_MAP: &[(&str, Option<&str>)] =
    &[("minimal", None), ("low", None), ("medium", None)];

pub(crate) const OPENCODE_GO_GLM52_THINKING_LEVEL_MAP: &[(&str, Option<&str>)] = &[
    ("off", None),
    ("minimal", None),
    ("low", None),
    ("medium", None),
    ("high", Some("high")),
    ("max", Some("max")),
];

pub(crate) const DEEPSEEK_V4_THINKING_LEVEL_MAP: &[(&str, Option<&str>)] = &[
    ("minimal", None),
    ("low", None),
    ("medium", None),
    ("high", Some("high")),
    ("max", Some("max")),
];
/// `DEEPSEEK_V4_FLASH_THINKING_LEVEL_MAP` (adds `low: "low"`).
pub(crate) const DEEPSEEK_V4_FLASH_THINKING_LEVEL_MAP: &[(&str, Option<&str>)] = &[
    ("minimal", None),
    ("low", Some("low")),
    ("medium", None),
    ("high", Some("high")),
    ("max", Some("max")),
];

pub(crate) const QWEN_TOKEN_PLAN_FALLBACK_THINKING_LEVEL_MAP: &[(&str, Option<&str>)] = &[
    ("minimal", None),
    ("low", None),
    ("medium", None),
    ("high", Some("high")),
    ("xhigh", None),
    ("max", Some("max")),
];

pub(crate) const ANT_LING_RING_THINKING_LEVEL_MAP: &[(&str, Option<&str>)] = &[
    ("off", None),
    ("minimal", None),
    ("low", None),
    ("medium", None),
    ("high", Some("high")),
    ("xhigh", Some("xhigh")),
];

// ---------------------------------------------------------------------------
// Compat literal constructors
// ---------------------------------------------------------------------------

/// `XAI_RESPONSES_COMPAT` (generate-models.ts:440-442).
pub(crate) fn xai_responses_compat() -> JsObj {
    obj! { "supportsLongCacheRetention" => Jv::b(false) }
}

/// `NVIDIA_OPENAI_COMPAT` (generate-models.ts:232-239).
pub(crate) fn nvidia_openai_compat() -> JsObj {
    obj! {
        "supportsStore" => Jv::b(false),
        "supportsDeveloperRole" => Jv::b(false),
        "supportsReasoningEffort" => Jv::b(false),
        "maxTokensField" => Jv::s("max_tokens"),
        "supportsStrictMode" => Jv::b(false),
        "supportsLongCacheRetention" => Jv::b(false),
    }
}

/// The `qwenTokenPlanCompat` literal (generate-models.ts:2470-2475).
pub(crate) fn qwen_token_plan_compat() -> JsObj {
    obj! {
        "thinkingFormat" => Jv::s("qwen"),
        "supportsDeveloperRole" => Jv::b(false),
        "supportsStore" => Jv::b(false),
        "supportsReasoningEffort" => Jv::b(true),
    }
}

/// The `moonshotCompat` literal (generate-models.ts:2357-2364).
pub(crate) fn moonshot_compat() -> JsObj {
    obj! {
        "supportsStore" => Jv::b(false),
        "supportsDeveloperRole" => Jv::b(false),
        "supportsReasoningEffort" => Jv::b(false),
        "maxTokensField" => Jv::s("max_tokens"),
        "supportsStrictMode" => Jv::b(false),
        "thinkingFormat" => Jv::s("deepseek"),
    }
}

/// The `xiaomiCompat` literal (generate-models.ts:2411-2414).
pub(crate) fn xiaomi_compat() -> JsObj {
    obj! {
        "requiresReasoningContentOnAssistantMessages" => Jv::b(true),
        "thinkingFormat" => Jv::s("deepseek"),
    }
}

/// `getModelsDevCost` (generate-models.ts:1162-1184): rates plus context
/// tiers. Tier key order follows the upstream literal.
pub(crate) fn get_models_dev_cost(model: &Json) -> JsObj {
    let cost = model.get("cost");
    let tiers: Vec<JsObj> = cost
        .and_then(|cost| cost.get("tiers"))
        .and_then(Json::as_array)
        .map(|tiers| {
            tiers
                .iter()
                .filter_map(|tier| {
                    let context = tier.get("tier")?;
                    if context.get("type").and_then(Json::as_str) != Some("context") {
                        return None;
                    }
                    let size = context.get("size").and_then(Json::as_f64)?;
                    Some(obj! {
                        "inputTokensAbove" => Jv::n(size),
                        "input" => Jv::n(js_or(tier.get("input").and_then(Json::as_f64), 0.0)),
                        "output" => Jv::n(js_or(tier.get("output").and_then(Json::as_f64), 0.0)),
                        "cacheRead" => Jv::n(js_or(tier.get("cache_read").and_then(Json::as_f64), 0.0)),
                        "cacheWrite" => Jv::n(js_or(tier.get("cache_write").and_then(Json::as_f64), 0.0)),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let mut result = obj! {
        "input" => Jv::n(js_or(cost.and_then(|cost| cost.get("input")).and_then(Json::as_f64), 0.0)),
        "output" => Jv::n(js_or(cost.and_then(|cost| cost.get("output")).and_then(Json::as_f64), 0.0)),
        "cacheRead" => Jv::n(js_or(cost.and_then(|cost| cost.get("cache_read")).and_then(Json::as_f64), 0.0)),
        "cacheWrite" => Jv::n(js_or(cost.and_then(|cost| cost.get("cache_write")).and_then(Json::as_f64), 0.0)),
    };
    if !tiers.is_empty() {
        result.set("tiers", Jv::Arr(tiers.into_iter().map(Jv::Obj).collect()));
    }
    result
}

/// `getGoogleThinkingLevelMap` (generate-models.ts:933-943).
pub(crate) fn google_thinking_level_map(model_id: &str, options: Option<&Json>) -> Option<JsObj> {
    if let Some(map) = effort_thinking_level_map(&parse_reasoning_options(options)) {
        return Some(map);
    }
    is_gemma4_model(model_id).then(|| {
        level_map(&[
            ("off", None),
            ("minimal", Some("MINIMAL")),
            ("low", None),
            ("medium", None),
            ("high", Some("HIGH")),
        ])
    })
}

/// `/gemma-?4/` (generate-models.ts:929-931).
fn is_gemma4_model(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    id.contains("gemma4") || id.contains("gemma-4")
}

/// `supportsOpenAiXhigh` (generate-models.ts:537-546).
fn supports_openai_xhigh(model_id: &str) -> bool {
    [
        "gpt-5.2",
        "gpt-5.3",
        "gpt-5.4",
        "gpt-5.5",
        "gpt-5.6",
        "gpt-6-astra",
    ]
    .iter()
    .any(|marker| model_id.contains(marker))
}

/// `supportsOpenAiMax` (generate-models.ts:548-556).
fn supports_openai_max(model: &JsObj) -> bool {
    let id = string_value(model, "id").unwrap_or_default();
    let api = string_value(model, "api").unwrap_or_default();
    (id.contains("gpt-5.6") || id.contains("gpt-6-astra"))
        && (api == "openai-responses"
            || api == "azure-openai-responses"
            || api == "openai-codex-responses"
            || api == "openai-completions")
}

/// `/^~?anthropic\//` (OpenRouter's Anthropic-prefixed model ids).
fn openrouter_anthropic_prefixed(model_id: &str) -> bool {
    model_id
        .strip_prefix('~')
        .unwrap_or(model_id)
        .starts_with("anthropic/")
}

/// `/^claude-opus-5(?:-\d{8})?$/` on the unprefixed lowercased id, or
/// `/^claude-(?:fable|mythos)-5(?:[.-]1)(?:-\d{8})?$/`.
pub(crate) fn supports_anthropic_mid_convo_effort(model_id: &str) -> bool {
    let id = model_id
        .strip_prefix('~')
        .unwrap_or(model_id)
        .strip_prefix("anthropic/")
        .unwrap_or(model_id)
        .to_lowercase();
    if let Some(rest) = id.strip_prefix("claude-opus-5") {
        return rest.is_empty() || is_yyyymmdd_suffix(rest);
    }
    for family in ["claude-fable-5", "claude-mythos-5"] {
        if let Some(rest) = id.strip_prefix(family) {
            let Some(rest) = rest.strip_prefix("-1").or_else(|| rest.strip_prefix(".1")) else {
                continue;
            };
            return rest.is_empty() || is_yyyymmdd_suffix(rest);
        }
    }
    false
}

fn is_yyyymmdd_suffix(rest: &str) -> bool {
    let Some(suffix) = rest.strip_prefix('-') else {
        return false;
    };
    suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit())
}

/// `/^claude-opus-(?:4[.-]8|5)(?:-\d{8})?$/` or
/// `/^claude-(?:fable|mythos)-5(?:[.-]1)?(?:-\d{8})?$/`.
pub(crate) fn supports_anthropic_mid_convo_system_messages(model_id: &str) -> bool {
    if let Some(rest) = model_id.strip_prefix("claude-opus-") {
        let rest = rest
            .strip_prefix("4.8")
            .or_else(|| rest.strip_prefix("4-8"))
            .or_else(|| rest.strip_prefix("5"));
        if let Some(rest) = rest {
            return rest.is_empty() || is_yyyymmdd_suffix(rest);
        }
    }
    for family in ["claude-fable-5", "claude-mythos-5"] {
        if let Some(rest) = model_id.strip_prefix(family) {
            let rest = rest
                .strip_prefix("-1")
                .or_else(|| rest.strip_prefix(".1"))
                .unwrap_or(rest);
            return rest.is_empty() || is_yyyymmdd_suffix(rest);
        }
    }
    false
}

/// `isAnthropicAdaptiveThinkingModel` (generate-models.ts:579-596).
fn is_anthropic_adaptive_thinking_model(model_id: &str) -> bool {
    [
        "opus-4-6",
        "opus-4.6",
        "opus-4-7",
        "opus-4.7",
        "opus-4-8",
        "opus-4.8",
        "opus-5",
        "opus.5",
        "sonnet-4-6",
        "sonnet-4.6",
        "sonnet-5",
        "sonnet.5",
        "fable-5",
        "mythos-5",
    ]
    .iter()
    .any(|marker| model_id.contains(marker))
}

/// `isAnthropicTemperatureUnsupportedModel` (generate-models.ts:598-608).
fn is_anthropic_temperature_unsupported_model(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    [
        "opus-4-7", "opus-4.7", "opus-4-8", "opus-4.8", "opus-5", "opus.5",
    ]
    .iter()
    .any(|marker| id.contains(marker))
}

/// `getAnthropicMessagesCompat` (generate-models.ts:1121-1146).
pub(crate) fn get_anthropic_messages_compat(provider: &str, model_id: &str) -> Option<JsObj> {
    let mut compat = JsObj::new();
    if set_contains(VERIFIED_ANTHROPIC_MID_CONVO_EFFORT_PROVIDERS, provider)
        && supports_anthropic_mid_convo_effort(model_id)
        && !set_contains(
            MID_CONVO_EFFORT_UNSUPPORTED_ANTHROPIC_MODELS,
            &format!("{provider}:{model_id}"),
        )
    {
        compat.set("supportsMidConvoEffort", Jv::b(true));
    }
    if provider == "anthropic" && supports_anthropic_mid_convo_system_messages(model_id) {
        compat.set("supportsMidConvoSystemMessages", Jv::b(true));
        compat.set("supportsMidConvoToolChanges", Jv::b(true));
    }
    // OpenCode Zen and GitHub Copilot forward mid-conversation system messages
    // but reject `tool_addition`/`tool_removal` blocks, so tool changes stay
    // top-level there.
    if (provider == "opencode" || provider == "github-copilot")
        && supports_anthropic_mid_convo_system_messages(model_id)
    {
        compat.set("supportsMidConvoSystemMessages", Jv::b(true));
    }
    if set_contains(
        EAGER_TOOL_INPUT_STREAMING_UNSUPPORTED_ANTHROPIC_MODELS,
        &format!("{provider}:{model_id}"),
    ) {
        compat.set("supportsEagerToolInputStreaming", Jv::b(false));
    }
    if provider == "xiaomi" || provider.starts_with("xiaomi-token-plan-") {
        compat.set("allowEmptySignature", Jv::b(true));
    }
    (!compat.is_empty()).then_some(compat)
}

/// `normalizeNvidiaModelId` (generate-models.ts:1154-1156).
pub(crate) fn normalize_nvidia_model_id(model_id: &str) -> String {
    model_id.to_lowercase().replace('_', ".")
}

// ---------------------------------------------------------------------------
// OpenAI-completions compat detection (generate-models.ts:610-774)
// ---------------------------------------------------------------------------

/// Upstream `OPENAI_COMPLETIONS_DEFAULT_COMPAT` (generate-models.ts:610-640),
/// in the literal's key order.
fn openai_completions_default_compat() -> JsObj {
    obj! {
        "supportsStore" => Jv::b(true),
        "supportsDeveloperRole" => Jv::b(true),
        "supportsReasoningEffort" => Jv::b(true),
        "supportsUsageInStreaming" => Jv::b(true),
        "supportsFinishReason" => Jv::b(true),
        "maxTokensField" => Jv::s("max_completion_tokens"),
        "requiresToolResultName" => Jv::b(false),
        "requiresAssistantAfterToolResult" => Jv::b(false),
        "requiresThinkingAsText" => Jv::b(false),
        "requiresReasoningContentOnAssistantMessages" => Jv::b(false),
        "thinkingFormat" => Jv::s("openai"),
        "openRouterRouting" => Jv::Obj(JsObj::new()),
        "vercelGatewayRouting" => Jv::Obj(JsObj::new()),
        "chatTemplateKwargs" => Jv::Obj(JsObj::new()),
        "chatTemplateArgs" => Jv::Obj(JsObj::new()),
        "zaiToolStream" => Jv::b(false),
        "supportsStrictMode" => Jv::b(true),
        "supportsOpenAIGrammarTools" => Jv::b(false),
        "supportsMidConvoSystemMessages" => Jv::b(false),
        "supportsMidConvoToolAdditions" => Jv::b(false),
        "sendSessionAffinityHeaders" => Jv::b(false),
        "supportsLongCacheRetention" => Jv::b(true),
    }
}

/// Upstream `detectOpenAICompletionsCompat` (generate-models.ts:650-745):
/// every resolved key in the return literal's order (the delta's key order
/// follows from this).
fn detect_openai_completions_compat(provider: &str, base_url: &str, model_id: &str) -> JsObj {
    let base_url_lower = base_url.to_lowercase();
    let is_zai = provider == "zai"
        || provider == "zai-coding-cn"
        || base_url.contains("api.z.ai")
        || base_url.contains("open.bigmodel.cn");
    let is_together = provider == "together"
        || base_url.contains("api.together.ai")
        || base_url.contains("api.together.xyz");
    let is_moonshot = provider == "moonshotai"
        || provider == "moonshotai-cn"
        || base_url.contains("api.moonshot.");
    let is_openrouter = provider == "openrouter" || base_url.contains("openrouter.ai");
    let is_cloudflare_workers_ai =
        provider == "cloudflare-workers-ai" || base_url.contains("api.cloudflare.com");
    let is_cloudflare_ai_gateway =
        provider == "cloudflare-ai-gateway" || base_url.contains("gateway.ai.cloudflare.com");
    let is_nvidia = provider == "nvidia" || base_url.contains("integrate.api.nvidia.com");
    let is_ant_ling = provider == "ant-ling" || base_url.contains("api.ant-ling.com");
    let is_together_reasoning_only =
        is_together && set_contains(TOGETHER_REASONING_ONLY_MODELS, model_id);
    let is_deepseek = provider == "deepseek" || base_url_lower.contains("deepseek.com");

    let is_non_standard = is_nvidia
        || provider == "cerebras"
        || base_url.contains("cerebras.ai")
        || provider == "xai"
        || base_url.contains("api.x.ai")
        || is_together
        || base_url.contains("chutes.ai")
        || is_deepseek
        || is_zai
        || is_moonshot
        || provider == "opencode"
        || base_url.contains("opencode.ai")
        || is_cloudflare_workers_ai
        || is_cloudflare_ai_gateway
        || is_ant_ling;

    let use_max_tokens = base_url.contains("chutes.ai")
        || is_deepseek
        || is_moonshot
        || is_cloudflare_ai_gateway
        || is_together
        || is_nvidia
        || is_ant_ling
        || is_zai;

    let is_grok = provider == "xai" || base_url.contains("api.x.ai");
    let is_openrouter_developer_role_model =
        is_openrouter && (model_id.starts_with("anthropic/") || model_id.starts_with("openai/"));

    let thinking_format = if is_deepseek {
        "deepseek"
    } else if is_zai {
        "zai"
    } else if is_together && !is_together_reasoning_only {
        "together"
    } else if is_ant_ling {
        "ant-ling"
    } else if is_openrouter {
        "openrouter"
    } else {
        "openai"
    };

    let mut compat = obj! {
        "supportsStore" => Jv::b(!is_non_standard),
        "supportsDeveloperRole" => Jv::b(is_openrouter_developer_role_model || (!is_non_standard && !is_openrouter)),
        "supportsReasoningEffort" => Jv::b(
            !is_grok && !is_zai && !is_moonshot && !is_together && !is_cloudflare_ai_gateway && !is_nvidia && !is_ant_ling,
        ),
        "supportsUsageInStreaming" => Jv::b(true),
        "supportsFinishReason" => Jv::b(true),
        "maxTokensField" => Jv::s(if use_max_tokens { "max_tokens" } else { "max_completion_tokens" }),
        "requiresToolResultName" => Jv::b(false),
        "requiresAssistantAfterToolResult" => Jv::b(false),
        "requiresThinkingAsText" => Jv::b(false),
        "requiresReasoningContentOnAssistantMessages" => Jv::b(is_deepseek),
        "thinkingFormat" => Jv::s(thinking_format),
        "openRouterRouting" => Jv::Obj(JsObj::new()),
        "vercelGatewayRouting" => Jv::Obj(JsObj::new()),
        "chatTemplateKwargs" => Jv::Obj(JsObj::new()),
        "chatTemplateArgs" => Jv::Obj(JsObj::new()),
        "zaiToolStream" => Jv::b(false),
        "supportsStrictMode" => Jv::b(!is_moonshot && !is_together && !is_cloudflare_ai_gateway && !is_nvidia),
        "supportsOpenAIGrammarTools" => Jv::b(false),
        "supportsMidConvoSystemMessages" => Jv::b(false),
        "supportsMidConvoToolAdditions" => Jv::b(false),
    };
    // ...(cacheControlFormat ? { cacheControlFormat } : {}) — positioned
    // between the mid-convo flags and sendSessionAffinityHeaders in the
    // upstream return literal, which fixes the delta's key order.
    if provider == "openrouter" && openrouter_anthropic_prefixed(model_id) {
        compat.set("cacheControlFormat", Jv::s("anthropic"));
    }
    compat
        .set("sendSessionAffinityHeaders", Jv::b(is_openrouter))
        .set(
            "supportsLongCacheRetention",
            Jv::b(
                !(is_together
                    || is_cloudflare_workers_ai
                    || is_cloudflare_ai_gateway
                    || is_nvidia
                    || is_ant_ling),
            ),
        );
    compat
}

/// Upstream `openAICompletionsCompatDelta`: the resolved compat reduced to the
/// keys that differ from the defaults (plain-empty objects on both sides are
/// skipped). Iteration order = the detected literal order.
fn openai_completions_compat_delta(compat: &JsObj) -> JsObj {
    let defaults = openai_completions_default_compat();
    let mut delta = JsObj::new();
    for (key, value) in &compat.entries {
        let default_value = defaults.get(key);
        if value.is_plain_empty_object() && default_value.is_some_and(Jv::is_plain_empty_object) {
            continue;
        }
        if Some(value) != default_value {
            delta.set(key, value.clone());
        }
    }
    delta
}

/// Upstream `applyOpenAICompletionsCompatMetadata` (generate-models.ts:767-774):
/// `{...detected, ...model.compat}`, dropping the key entirely when the merge
/// is empty.
fn apply_openai_completions_compat_metadata(model: &mut JsObj) {
    if string_value(model, "api") != Some("openai-completions") {
        return;
    }
    let detected = openai_completions_compat_delta(&detect_openai_completions_compat(
        string_value(model, "provider").unwrap_or_default(),
        string_value(model, "baseUrl").unwrap_or_default(),
        string_value(model, "id").unwrap_or_default(),
    ));
    let mut merged = detected;
    if let Some(existing) = compat_obj(model) {
        merged.spread(existing);
    }
    if merged.is_empty() {
        model.remove("compat");
    } else {
        model.set("compat", Jv::Obj(merged));
    }
}

/// Upstream `supportsDirectReasoningEffort` (generate-models.ts:493-509).
fn supports_direct_reasoning_effort(model: &JsObj) -> bool {
    let api = string_value(model, "api").unwrap_or_default();
    if api == "anthropic-messages" {
        return compat_flag(model, "forceAdaptiveThinking") == Some(true);
    }
    if api == "openai-responses"
        || api == "azure-openai-responses"
        || api == "openai-codex-responses"
    {
        return true;
    }
    if api != "openai-completions" {
        return false;
    }
    let mut compat = detect_openai_completions_compat(
        string_value(model, "provider").unwrap_or_default(),
        string_value(model, "baseUrl").unwrap_or_default(),
        string_value(model, "id").unwrap_or_default(),
    );
    if let Some(existing) = compat_obj(model) {
        compat.spread(existing);
    }
    compat.get("thinkingFormat").and_then(Jv::as_str) == Some("openai")
        && compat.get("supportsReasoningEffort") == Some(&Jv::b(true))
}

/// Upstream `applyModelsDevReasoningOptionMetadata` (generate-models.ts:511-516).
fn apply_models_dev_reasoning_option_metadata(
    model: &mut JsObj,
    recorded: &HashMap<String, Vec<ReasoningOption>>,
) {
    let key = format!(
        "{}:{}",
        string_value(model, "provider").unwrap_or_default(),
        string_value(model, "id").unwrap_or_default()
    );
    let Some(options) = recorded.get(&key) else {
        return;
    };
    if !supports_direct_reasoning_effort(model) {
        return;
    }
    if let Some(map) = effort_thinking_level_map(options) {
        merge_thinking_level_map(model, map);
    }
}

/// Upstream `applyAnthropicMessagesCompatMetadata` (generate-models.ts:776-783).
fn apply_anthropic_messages_compat_metadata(model: &mut JsObj) {
    if string_value(model, "api") != Some("anthropic-messages") {
        return;
    }
    let compat = get_anthropic_messages_compat(
        string_value(model, "provider").unwrap_or_default(),
        string_value(model, "id").unwrap_or_default(),
    );
    if let Some(compat) = compat {
        let supports_mid_convo_effort = compat.get("supportsMidConvoEffort") == Some(&Jv::b(true));
        merge_compat(model, compat);
        if supports_mid_convo_effort {
            merge_thinking_level_map(model, level_map(&[("off", None)]));
        }
    }
}

/// Upstream `applyStrictToolCompatMetadata` (generate-models.ts:814-823).
fn apply_strict_tool_compat_metadata(model: &mut JsObj) {
    let provider = string_value(model, "provider").unwrap_or_default();
    let api = string_value(model, "api").unwrap_or_default();
    if (provider == "openai" || provider == "cloudflare-ai-gateway") && api == "openai-responses" {
        merge_compat(model, obj! { "supportsStrictMode" => Jv::b(true) });
    } else if provider == "anthropic" && api == "anthropic-messages" {
        merge_compat(model, obj! { "supportsStrictTools" => Jv::b(true) });
    }
}

/// Upstream `applyOpenAIGrammarToolCompatMetadata` (generate-models.ts:843-848):
/// OpenAI rejects `type: "custom"` tools for pre-GPT-5 models.
fn apply_openai_grammar_tool_compat_metadata(model: &mut JsObj) {
    let provider = string_value(model, "provider").unwrap_or_default();
    let api = string_value(model, "api").unwrap_or_default();
    if !set_contains(OPENAI_GRAMMAR_TOOL_APIS, api)
        || !set_contains(OPENAI_GRAMMAR_TOOL_PROVIDERS, provider)
    {
        return;
    }
    let Some(rest) = string_value(model, "id")
        .unwrap_or_default()
        .strip_prefix("gpt-")
    else {
        return;
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let Ok(major) = digits.parse::<i64>() else {
        return;
    };
    if major < 5 {
        return;
    }
    merge_compat(model, obj! { "supportsOpenAIGrammarTools" => Jv::b(true) });
}

/// Upstream `applyOpenAIToolSearchMetadata` (generate-models.ts:850-862).
fn apply_openai_tool_search_metadata(model: &mut JsObj) {
    let provider = string_value(model, "provider").unwrap_or_default();
    let api = string_value(model, "api").unwrap_or_default();
    let id = string_value(model, "id").unwrap_or_default();
    let is_openai_responses = provider == "openai" && api == "openai-responses";
    let is_openai_codex = provider == "openai-codex" && api == "openai-codex-responses";
    if !(is_openai_responses || is_openai_codex) || !set_contains(OPENAI_TOOL_SEARCH_MODEL_IDS, id)
    {
        return;
    }
    let supports_additional_tools = (is_openai_responses
        && set_contains(OPENAI_TOOL_SEARCH_MODEL_IDS, id))
        || (is_openai_codex && set_contains(OPENAI_CODEX_ADDITIONAL_TOOLS_MODEL_IDS, id));
    let mut compat = JsObj::new();
    if supports_additional_tools {
        compat.set("supportsAdditionalTools", Jv::b(true));
    }
    compat.set("supportsToolSearch", Jv::b(true));
    merge_compat(model, compat);
}

/// Upstream `applyOpenAICompletionsTranscriptMetadata` (generate-models.ts:869-891).
fn apply_openai_completions_transcript_metadata(model: &mut JsObj) {
    if string_value(model, "api") != Some("openai-completions") {
        return;
    }
    let provider = string_value(model, "provider").unwrap_or_default();
    let id = string_value(model, "id").unwrap_or_default();
    let is_kimi_k3 = (provider.starts_with("moonshot") && id == "kimi-k3")
        || (provider == "fireworks" && id.contains("kimi-k3"))
        || ((provider == "opencode" || provider == "opencode-go") && id == "kimi-k3");
    let is_text_only = (provider.starts_with("moonshot")
        && (id == "kimi-k2.6" || id == "kimi-k2.7-code" || id == "kimi-k2.7-code-highspeed"))
        || (provider == "github-copilot" && id == "kimi-k3")
        || (provider == "deepseek" && id == "deepseek-v4-pro")
        || (provider == "openrouter"
            && id.starts_with("openai/")
            && set_contains(
                OPENAI_TOOL_SEARCH_MODEL_IDS,
                id.strip_prefix("openai/").unwrap_or_default(),
            ));
    if !is_kimi_k3 && !is_text_only {
        return;
    }
    let mut compat = obj! { "supportsMidConvoSystemMessages" => Jv::b(true) };
    if is_kimi_k3 {
        compat.set("supportsMidConvoToolAdditions", Jv::b(true));
    }
    merge_compat(model, compat);
}

/// Upstream `applyOpenAIResponsesTranscriptMetadata` (generate-models.ts:899-915).
fn apply_openai_responses_transcript_metadata(model: &mut JsObj) {
    let provider = string_value(model, "provider").unwrap_or_default();
    let api = string_value(model, "api").unwrap_or_default();
    let id = string_value(model, "id").unwrap_or_default();
    let is_openai_responses = provider == "openai" && api == "openai-responses";
    let is_openai_codex = provider == "openai-codex" && api == "openai-codex-responses";
    let is_proxied_responses =
        set_contains(OPENAI_RESPONSES_PROXY_PROVIDERS, provider) && api == "openai-responses";
    if !(is_openai_responses || is_openai_codex || is_proxied_responses)
        || !set_contains(OPENAI_TOOL_SEARCH_MODEL_IDS, id)
    {
        return;
    }
    let mut compat = obj! { "supportsMidConvoSystemMessages" => Jv::b(true) };
    if is_proxied_responses {
        compat.set("supportsAdditionalTools", Jv::b(true));
    }
    merge_compat(model, compat);
}

/// Upstream `applyOpenAIExplicitPromptCacheMetadata` (generate-models.ts:920-927):
/// exactly the models accepting `prompt_cache_options`.
fn apply_openai_explicit_prompt_cache_metadata(model: &mut JsObj) {
    if string_value(model, "provider") != Some("openai")
        || string_value(model, "api") != Some("openai-responses")
    {
        return;
    }
    if cost_field(model, "cacheWrite") <= 0.0 {
        return;
    }
    merge_compat(
        model,
        obj! { "supportsExplicitPromptCacheMode" => Jv::b(true) },
    );
}

/// Upstream `applyThinkingLevelMetadata` (generate-models.ts:945-1119), in the
/// upstream statement order (insertion order of thinkingLevelMap keys follows
/// from it).
fn apply_thinking_level_metadata(
    model: &mut JsObj,
    recorded: &HashMap<String, Vec<ReasoningOption>>,
) {
    let provider = string_value(model, "provider")
        .unwrap_or_default()
        .to_string();
    let api = string_value(model, "api").unwrap_or_default().to_string();
    let id = string_value(model, "id").unwrap_or_default().to_string();

    if (api == "openai-responses" || api == "azure-openai-responses") && id.starts_with("gpt-5") {
        merge_thinking_level_map(model, level_map(&[("off", None)]));
    }
    if id == "gpt-6-astra"
        && (api == "openai-responses"
            || api == "azure-openai-responses"
            || api == "openai-codex-responses")
    {
        merge_thinking_level_map(
            model,
            level_map(&[
                ("off", None),
                ("minimal", None),
                ("low", Some("low")),
                ("medium", Some("medium")),
                ("high", Some("high")),
                ("xhigh", Some("xhigh")),
                ("max", Some("max")),
            ]),
        );
    }
    if provider == "github-copilot" && id.starts_with("gpt-5") {
        merge_thinking_level_map(model, level_map(&[("minimal", Some("low"))]));
    }
    if api == "openai-responses"
        && provider == "openai"
        && set_contains(OPENAI_RESPONSES_NONE_REASONING_MODELS, &id)
    {
        merge_thinking_level_map(model, level_map(&[("off", Some("none"))]));
    }
    // xAI models without verified effort options must not send the
    // undocumented "none"/"minimal" efforts.
    if provider == "xai" && api == "openai-responses" && model.get("thinkingLevelMap").is_none() {
        merge_thinking_level_map(model, level_map(&[("off", None), ("minimal", None)]));
    }
    if supports_openai_xhigh(&id) {
        merge_thinking_level_map(model, level_map(&[("xhigh", Some("xhigh"))]));
    }
    if supports_openai_max(model) {
        merge_thinking_level_map(model, level_map(&[("max", Some("max"))]));
    }
    if provider == "openai" && id == "gpt-5.5" {
        merge_thinking_level_map(model, level_map(&[("minimal", None)]));
    }
    if id.ends_with("gpt-5.5-pro") {
        merge_thinking_level_map(
            model,
            level_map(&[("off", None), ("minimal", None), ("low", None)]),
        );
    }
    // Anthropic adaptive-thinking effort support (per Anthropic adaptive
    // thinking docs): "max" is available on all adaptive-thinking Claude
    // models; "xhigh" only on Opus 4.7/4.8/5, Sonnet 5, and Fable 5.
    if id.contains("opus-4-6")
        || id.contains("opus-4.6")
        || id.contains("sonnet-4-6")
        || id.contains("sonnet-4.6")
    {
        merge_thinking_level_map(model, level_map(&[("max", Some("max"))]));
    }
    if id.contains("opus-4-7")
        || id.contains("opus-4.7")
        || id.contains("opus-4-8")
        || id.contains("opus-4.8")
        || id.contains("opus-5")
        || id.contains("opus.5")
        || id.contains("sonnet-5")
        || id.contains("sonnet.5")
    {
        merge_thinking_level_map(
            model,
            level_map(&[("xhigh", Some("xhigh")), ("max", Some("max"))]),
        );
    }
    if id.contains("fable-5") {
        merge_thinking_level_map(
            model,
            level_map(&[
                ("off", None),
                ("xhigh", Some("xhigh")),
                ("max", Some("max")),
            ]),
        );
    }
    if api == "anthropic-messages" && is_anthropic_adaptive_thinking_model(&id) {
        merge_compat(model, obj! { "forceAdaptiveThinking" => Jv::b(true) });
    }
    if api == "anthropic-messages" && is_anthropic_temperature_unsupported_model(&id) {
        merge_compat(model, obj! { "supportsTemperature" => Jv::b(false) });
    }
    if api == "openai-completions"
        && id.contains("deepseek-v4")
        && model.get("thinkingLevelMap").is_none()
    {
        let map = if provider == "openrouter" {
            level_map(&[
                ("minimal", None),
                ("low", None),
                ("medium", None),
                ("high", Some("high")),
                ("xhigh", Some("xhigh")),
                ("max", None),
            ])
        } else if (provider == "deepseek" || provider == "opencode" || provider == "opencode-go")
            && id.contains("deepseek-v4-flash")
        {
            level_map(DEEPSEEK_V4_FLASH_THINKING_LEVEL_MAP)
        } else {
            level_map(DEEPSEEK_V4_THINKING_LEVEL_MAP)
        };
        merge_thinking_level_map(model, map);
    }
    if provider == "groq" && id == "qwen/qwen3.6-27b" {
        merge_thinking_level_map(
            model,
            level_map(&[
                ("minimal", None),
                ("low", None),
                ("medium", None),
                ("high", Some("default")),
            ]),
        );
    }
    if provider == "openai-codex" && supports_openai_xhigh(&id) {
        merge_thinking_level_map(model, level_map(&[("minimal", Some("low"))]));
    }
    if (provider == "moonshotai" || provider == "moonshotai-cn")
        && (id == "kimi-k2.7-code" || id == "kimi-k2.7-code-highspeed")
    {
        // Kimi K2.7 Code is always-thinking. Official docs say
        // `thinking: { type: "disabled" }` is rejected, and callers can omit
        // the thinking parameter to use the enabled default.
        merge_thinking_level_map(model, level_map(&[("off", None)]));
    }
    if provider == "openrouter" && id.starts_with("inception/mercury-2") {
        // Mercury 2 in instant mode (reasoning_effort: "none") disables tool
        // calling; mark "off" unsupported so the openai-completions provider
        // omits the reasoning param.
        merge_thinking_level_map(model, level_map(&[("off", None)]));
    }
    if provider == "openrouter" && id == "z-ai/glm-5.2" {
        merge_thinking_level_map(model, level_map(&[("xhigh", Some("xhigh"))]));
    }
    if provider == "fireworks" {
        if api == "anthropic-messages" && compat_flag(model, "forceAdaptiveThinking") == Some(true)
        {
            // Qwen Max currently advertises only a toggle. Prefer upstream
            // effort metadata once available instead of replacing it with
            // this fallback.
            if id == "accounts/fireworks/models/qwen3p8-max"
                && model.get("thinkingLevelMap").is_none()
            {
                if let Some(map) = effort_thinking_level_map(&[ReasoningOption::Effort(vec![
                    Some("low".to_string()),
                    Some("medium".to_string()),
                    Some("xhigh".to_string()),
                ])]) {
                    model.set("thinkingLevelMap", Jv::Obj(map));
                }
            }
            let has_toggle = recorded
                .get(&recorded_model_key(model))
                .is_some_and(|options| options.contains(&ReasoningOption::Toggle));
            if has_toggle ||
                // The 2.4T alias omits the verified toggle in models.dev.
                id == "accounts/fireworks/models/qwen3p8-2p4t-a95b"
            {
                merge_thinking_level_map(model, level_map(&[("off", Some("none"))]));
            }
            if id == "accounts/fireworks/models/deepseek-v4-pro-0813" {
                merge_thinking_level_map(model, level_map(&[("low", Some("low"))]));
            }
        }
        if id.contains("glm-5p2") {
            // GLM 5.2 and its fast router support off/high/max. Fireworks maps
            // low and medium to high, so do not expose those aliases as
            // distinct levels.
            merge_thinking_level_map(
                model,
                level_map(&[
                    ("off", Some("none")),
                    ("minimal", None),
                    ("low", None),
                    ("medium", None),
                    ("max", Some("max")),
                ]),
            );
        }
        if id.contains("kimi-k3") {
            // Fireworks maps medium to high on both APIs; do not expose it as
            // a distinct level.
            merge_thinking_level_map(model, level_map(&[("medium", None)]));
        }
    }
    if provider == "opencode-go" && id == "glm-5.2" {
        merge_thinking_level_map(model, level_map(OPENCODE_GO_GLM52_THINKING_LEVEL_MAP));
    }
    if provider == "opencode-go" && id == "kimi-k2.6" {
        // OpenCode Go exposes Kimi K2.6 thinking as on/off, not distinct
        // effort tiers.
        merge_thinking_level_map(
            model,
            level_map(&[("minimal", None), ("low", None), ("medium", None)]),
        );
    }
    if provider == "opencode" && id == "grok-build-0.1" {
        // OpenCode Zen Grok Build reasons by default but rejects explicit
        // reasoningEffort.
        merge_thinking_level_map(
            model,
            level_map(&[
                ("off", None),
                ("minimal", None),
                ("low", None),
                ("medium", None),
            ]),
        );
    }
    if provider == "ant-ling" && model.get("reasoning") == Some(&Jv::b(true)) {
        // Ring reasons by default. Only high/xhigh have documented explicit
        // effort controls.
        merge_thinking_level_map(model, level_map(ANT_LING_RING_THINKING_LEVEL_MAP));
    }
    if provider == "github-copilot" {
        if let Some((_, override_map)) = GITHUB_COPILOT_THINKING_LEVEL_OVERRIDES
            .iter()
            .find(|(model_key, _)| model_key == &id)
        {
            merge_thinking_level_map(model, level_map(override_map));
        }
    }
}

/// The `provider:id` key upstream `getModelKey` builds for the recorded
/// reasoning-options map.
fn recorded_model_key(model: &JsObj) -> String {
    format!(
        "{}:{}",
        string_value(model, "provider").unwrap_or_default(),
        string_value(model, "id").unwrap_or_default()
    )
}

/// Upstream `applyAnthropicAllowedFallbackModelMetadata` (793-812): wire the
/// anthropic fallback graph with each fallback model's cost.
fn apply_anthropic_allowed_fallback_metadata(models: &mut [JsObj]) {
    let snapshot: Vec<(String, JsObj)> = models
        .iter()
        .filter(|model| is_anthropic_fallback_metadata_model(model))
        .map(|model| {
            (
                string_value(model, "id").unwrap_or_default().to_string(),
                model.clone(),
            )
        })
        .collect();

    // Collect the compat additions first, then apply (upstream mutates the
    // same objects it reads, but only the fallbacks' costs are read, which
    // this pass does not change).
    let mut additions: Vec<(String, JsObj)> = Vec::new();
    for (model_id, fallback_model_ids) in ANTHROPIC_ALLOWED_FALLBACK_MODELS {
        let Some((_, primary)) = snapshot.iter().find(|(id, _)| id == model_id) else {
            continue;
        };
        let compatible: Vec<&str> = if compat_flag(primary, "supportsMidConvoEffort") == Some(true)
        {
            fallback_model_ids
                .iter()
                .copied()
                .filter(|id| supports_anthropic_mid_convo_effort(id))
                .collect()
        } else {
            fallback_model_ids.to_vec()
        };
        let allowed: Vec<JsObj> = compatible
            .iter()
            .filter_map(|fallback_model_id| {
                models
                    .iter()
                    .find(|model| string_value(model, "id") == Some(*fallback_model_id))
                    .map(|fallback| {
                        obj! {
                            "provider" => Jv::s(string_value(fallback, "provider").unwrap_or_default()),
                            "model" => Jv::s(*fallback_model_id),
                            "cost" => fallback.get("cost").cloned().unwrap_or(Jv::Null),
                        }
                    })
            })
            .collect();
        if !allowed.is_empty() {
            additions.push((
                model_id.to_string(),
                obj! {
                    "allowedFallbackModels" => Jv::Arr(allowed.into_iter().map(Jv::Obj).collect()),
                },
            ));
        }
    }
    for (model_id, compat) in additions {
        if let Some(model) = models
            .iter_mut()
            .find(|model| string_value(model, "id") == Some(model_id.as_str()))
        {
            merge_compat(model, compat);
        }
    }
}

/// Upstream `isAnthropicFallbackMetadataModel` (785-791).
fn is_anthropic_fallback_metadata_model(model: &JsObj) -> bool {
    if string_value(model, "provider") != Some("anthropic")
        || string_value(model, "api") != Some("anthropic-messages")
    {
        return false;
    }
    let id = string_value(model, "id").unwrap_or_default();
    ANTHROPIC_ALLOWED_FALLBACK_MODELS
        .iter()
        .any(|(primary, fallbacks)| *primary == id || fallbacks.contains(&id))
}

// ---------------------------------------------------------------------------
// Overrides and additive passes (generate-models.ts:2560-3023)
// ---------------------------------------------------------------------------

/// Upstream `withOpenAiLongContextPricing` (generate-models.ts:397-410).
fn with_openai_long_context_pricing(cost: &JsObj) -> JsObj {
    let rate = |field: &str| cost.get(field).and_then(Jv::as_f64).unwrap_or(0.0);
    let mut priced = cost.clone();
    priced.set(
        "tiers",
        Jv::Arr(vec![Jv::Obj(obj! {
            "inputTokensAbove" => Jv::n(OPENAI_LONG_CONTEXT_INPUT_THRESHOLD),
            "input" => Jv::n(round_cost(rate("input") * 2.0)),
            "output" => Jv::n(round_cost(rate("output") * 1.5)),
            "cacheRead" => Jv::n(round_cost(rate("cacheRead") * 2.0)),
            "cacheWrite" => Jv::n(round_cost(rate("cacheWrite") * 2.0)),
        })]),
    );
    priced
}

/// `OPENAI_GPT_56_STANDARD_COSTS[id]` as a cost object.
fn openai_standard_cost(model_id: &str) -> Option<JsObj> {
    OPENAI_GPT_56_STANDARD_COSTS
        .iter()
        .find(|(id, _)| *id == model_id)
        .map(|(_, cost)| {
            obj! {
                "input" => Jv::n(cost[0].1),
                "output" => Jv::n(cost[1].1),
                "cacheRead" => Jv::n(cost[2].1),
                "cacheWrite" => Jv::n(cost[3].1),
            }
        })
}

/// The temporary override pass (generate-models.ts:2566-2641) until upstream
/// model metadata is corrected.
fn apply_model_overrides(models: &mut [JsObj]) {
    for model in models.iter_mut() {
        let provider = string_value(model, "provider")
            .unwrap_or_default()
            .to_string();
        let id = string_value(model, "id").unwrap_or_default().to_string();

        if provider == "github-copilot" && set_contains(GITHUB_COPILOT_EXTENDED_CONTEXT_MODELS, &id)
        {
            set_numeric(model, "contextWindow", 1_000_000.0);
        }

        if (provider == "anthropic" || provider == "opencode" || provider == "opencode-go")
            && (id == "claude-opus-4-6"
                || id == "claude-sonnet-4-6"
                || id == "claude-opus-4.6"
                || id == "claude-sonnet-4.6")
        {
            set_numeric(model, "contextWindow", 1_000_000.0);
        }

        // OpenCode variants list Claude Sonnet 4/4.5 with 1M context, actual
        // limit is 200K
        if (provider == "opencode" || provider == "opencode-go")
            && (id == "claude-sonnet-4-5" || id == "claude-sonnet-4")
        {
            set_numeric(model, "contextWindow", 200_000.0);
        }
        if (provider == "opencode" || provider == "opencode-go") && id == "gpt-5.4" {
            set_numeric(model, "contextWindow", 272_000.0);
            set_numeric(model, "maxTokens", 128_000.0);
        }
        // Keep direct OpenAI requests in the short-context pricing tier by
        // default. Users can opt into the larger context through model
        // overrides, so retain long-context cost metadata on the capped
        // models.
        if provider == "openai" && set_contains(OPENAI_SHORT_CONTEXT_CAPPED_MODEL_IDS, &id) {
            set_numeric(model, "contextWindow", OPENAI_LONG_CONTEXT_INPUT_THRESHOLD);
            set_numeric(model, "maxTokens", 128_000.0);
        }
        if provider == "openai" && set_contains(OPENAI_LONG_CONTEXT_PRICING_MODEL_IDS, &id) {
            let cost = openai_standard_cost(&id).unwrap_or_else(|| {
                model
                    .get("cost")
                    .and_then(|cost| match cost {
                        Jv::Obj(cost) => Some(cost.clone()),
                        _ => None,
                    })
                    .unwrap_or_default()
            });
            model.set("cost", Jv::Obj(with_openai_long_context_pricing(&cost)));
        }
        // Cloudflare AI Gateway passes OpenAI usage through at OpenAI list
        // prices.
        if provider == "cloudflare-ai-gateway" {
            if let Some(standard_cost) = openai_standard_cost(&id) {
                model.set(
                    "cost",
                    Jv::Obj(with_openai_long_context_pricing(&standard_cost)),
                );
            }
        }
        // models.dev reports gpt-5-pro output as 272000 (a duplicate of the
        // input sub-limit); the actual max output is 128000.
        if provider == "openai" && id == "gpt-5-pro" {
            set_numeric(model, "maxTokens", 128_000.0);
        }
        // Keep Kimi K3's canonical output limit when gateway metadata is
        // missing or incorrect.
        if (provider == "openrouter" && set_contains(OPENROUTER_KIMI_K3_MODEL_IDS, &id))
            || (provider == "vercel-ai-gateway" && id == "moonshotai/kimi-k3")
        {
            set_numeric(model, "maxTokens", KIMI_K3_MAX_TOKENS);
        }
        // Keep selected OpenRouter model metadata stable until upstream
        // settles.
        if provider == "openrouter" && id == "moonshotai/kimi-k2.5" {
            set_cost_field(model, "input", 0.41);
            set_cost_field(model, "output", 2.06);
            set_cost_field(model, "cacheRead", 0.07);
            set_numeric(model, "maxTokens", 4096.0);
        }
        if provider == "openrouter" && id.starts_with("moonshotai/kimi-k2.6") {
            merge_compat(
                model,
                obj! {
                    "supportsDeveloperRole" => Jv::b(false),
                    "requiresReasoningContentOnAssistantMessages" => Jv::b(true),
                },
            );
        }
        if provider == "openrouter" && id == "z-ai/glm-5" {
            set_cost_field(model, "input", 0.6);
            set_cost_field(model, "output", 1.9);
            set_cost_field(model, "cacheRead", 0.119);
        }
    }
}

/// The `deepseekCompat` literal (generate-models.ts:2717-2720).
fn deepseek_compat() -> JsObj {
    obj! {
        "requiresReasoningContentOnAssistantMessages" => Jv::b(true),
        "thinkingFormat" => Jv::s("deepseek"),
    }
}

/// The `antLingCompat` literal (generate-models.ts:2764-2770).
fn ant_ling_compat() -> JsObj {
    obj! {
        "supportsStore" => Jv::b(false),
        "supportsDeveloperRole" => Jv::b(false),
        "supportsReasoningEffort" => Jv::b(false),
        "maxTokensField" => Jv::s("max_tokens"),
        "supportsLongCacheRetention" => Jv::b(false),
    }
}

/// The static entry fields shared by both literal orders below.
struct StaticEntry<'a> {
    id: &'a str,
    name: &'a str,
    api: &'a str,
    base_url: &'a str,
    provider: &'a str,
    reasoning: bool,
    input: Jv,
    cost: JsObj,
    context_window: f64,
    max_tokens: f64,
}

/// The missing-OpenAI-models literal (generate-models.ts:2644-2710) puts
/// `baseUrl` before `provider`.
fn static_openai_entry(entry: StaticEntry) -> JsObj {
    obj! {
        "id" => Jv::s(entry.id),
        "name" => Jv::s(entry.name),
        "api" => Jv::s(entry.api),
        "baseUrl" => Jv::s(entry.base_url),
        "provider" => Jv::s(entry.provider),
        "reasoning" => Jv::b(entry.reasoning),
        "input" => entry.input,
        "cost" => Jv::Obj(entry.cost),
        "contextWindow" => Jv::n(entry.context_window),
        "maxTokens" => Jv::n(entry.max_tokens),
    }
}

/// The codex/mistral/alias literals put `provider` before `baseUrl`
/// (generate-models.ts:2853-2971, 2931-2948).
fn static_codex_entry(entry: StaticEntry) -> JsObj {
    obj! {
        "id" => Jv::s(entry.id),
        "name" => Jv::s(entry.name),
        "api" => Jv::s(entry.api),
        "provider" => Jv::s(entry.provider),
        "baseUrl" => Jv::s(entry.base_url),
        "reasoning" => Jv::b(entry.reasoning),
        "input" => entry.input,
        "cost" => Jv::Obj(entry.cost),
        "contextWindow" => Jv::n(entry.context_window),
        "maxTokens" => Jv::n(entry.max_tokens),
    }
}

/// The static/additive model passes (generate-models.ts:2643-3023).
fn apply_additive_passes(models: &mut Vec<JsObj>) {
    // Add missing gpt models
    let long_context_cost = |input: f64, output: f64, cache_read: f64, cache_write: f64| {
        with_openai_long_context_pricing(&obj! {
            "input" => Jv::n(input),
            "output" => Jv::n(output),
            "cacheRead" => Jv::n(cache_read),
            "cacheWrite" => Jv::n(cache_write),
        })
    };
    let missing_openai_models: Vec<JsObj> = vec![
        static_openai_entry(StaticEntry {
            id: "gpt-6-astra",
            name: "GPT-6 Astra",
            api: "openai-responses",
            base_url: "https://api.openai.com/v1",
            provider: "openai",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: long_context_cost(10.0, 50.0, 1.0, 12.5),
            context_window: OPENAI_LONG_CONTEXT_INPUT_THRESHOLD,
            max_tokens: 128_000.0,
        }),
        static_openai_entry(StaticEntry {
            id: "gpt-5.6-sol",
            name: "GPT-5.6 Sol",
            api: "openai-responses",
            base_url: "https://api.openai.com/v1",
            provider: "openai",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: long_context_cost(5.0, 30.0, 0.5, 6.25),
            context_window: OPENAI_LONG_CONTEXT_INPUT_THRESHOLD,
            max_tokens: 128_000.0,
        }),
        static_openai_entry(StaticEntry {
            id: "gpt-5.6-terra",
            name: "GPT-5.6 Terra",
            api: "openai-responses",
            base_url: "https://api.openai.com/v1",
            provider: "openai",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: long_context_cost(2.0, 12.0, 0.2, 2.5),
            context_window: OPENAI_LONG_CONTEXT_INPUT_THRESHOLD,
            max_tokens: 128_000.0,
        }),
        static_openai_entry(StaticEntry {
            id: "gpt-5.6-luna",
            name: "GPT-5.6 Luna",
            api: "openai-responses",
            base_url: "https://api.openai.com/v1",
            provider: "openai",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: long_context_cost(0.2, 1.2, 0.02, 0.25),
            context_window: OPENAI_LONG_CONTEXT_INPUT_THRESHOLD,
            max_tokens: 128_000.0,
        }),
        static_openai_entry(StaticEntry {
            id: "gpt-5-chat-latest",
            name: "GPT-5 Chat Latest",
            api: "openai-responses",
            base_url: "https://api.openai.com/v1",
            provider: "openai",
            reasoning: false,
            input: Jv::str_list(&["text", "image"]),
            cost: obj! {
                "input" => Jv::n(1.25),
                "output" => Jv::n(10.0),
                "cacheRead" => Jv::n(0.125),
                "cacheWrite" => Jv::n(0.0),
            },
            context_window: 128_000.0,
            max_tokens: 16_384.0,
        }),
    ];
    for model in missing_openai_models {
        let id = string_value(&model, "id").unwrap_or_default().to_string();
        if !models.iter().any(|candidate| {
            string_value(candidate, "provider") == Some("openai")
                && string_value(candidate, "id") == Some(id.as_str())
        }) {
            models.push(model);
        }
    }

    let deepseek_compat = deepseek_compat();
    models.push(obj! {
        "id" => Jv::s("deepseek-flash"),
        "name" => Jv::s("DeepSeek V4.1 Flash"),
        "api" => Jv::s("openai-completions"),
        "baseUrl" => Jv::s("https://api.deepseek.com"),
        "provider" => Jv::s("deepseek"),
        "reasoning" => Jv::b(true),
        "thinkingLevelMap" => Jv::Obj(level_map(DEEPSEEK_V4_FLASH_THINKING_LEVEL_MAP)),
        "input" => Jv::str_list(&["text", "image"]),
        "cost" => Jv::Obj(obj! {
            // DeepSeek also offers time-based off-peak rates, which the cost
            // schema cannot represent yet.
            "input" => Jv::n(0.3),
            "output" => Jv::n(1.2),
            "cacheRead" => Jv::n(0.006),
            "cacheWrite" => Jv::n(0.0),
        }),
        "contextWindow" => Jv::n(1_000_000.0),
        "maxTokens" => Jv::n(384_000.0),
        "compat" => Jv::Obj(deepseek_compat.clone()),
    });
    models.push(obj! {
        "id" => Jv::s("deepseek-v4-pro"),
        "name" => Jv::s("DeepSeek V4 Pro"),
        "api" => Jv::s("openai-completions"),
        "baseUrl" => Jv::s("https://api.deepseek.com"),
        "provider" => Jv::s("deepseek"),
        "reasoning" => Jv::b(true),
        "input" => Jv::str_list(&["text"]),
        "cost" => Jv::Obj(obj! {
            "input" => Jv::n(1.32),
            "output" => Jv::n(3.96),
            "cacheRead" => Jv::n(0.044),
            "cacheWrite" => Jv::n(0.0),
        }),
        "contextWindow" => Jv::n(1_000_000.0),
        "maxTokens" => Jv::n(384_000.0),
        "compat" => Jv::Obj(deepseek_compat.clone()),
        // thinkingLevelMap arrives from the deepseek-v4 completions loop
        // below (DEEPSEEK_V4_THINKING_LEVEL_MAP), like upstream where the
        // literal omits it.
    });

    let ant_ling_compat = ant_ling_compat();
    let ant_ling_entry = |id: &str, name: &str, reasoning: bool, cost: [f64; 4], compat: JsObj| {
        obj! {
            "id" => Jv::s(id),
            "name" => Jv::s(name),
            "api" => Jv::s("openai-completions"),
            "baseUrl" => Jv::s("https://api.ant-ling.com/v1"),
            "provider" => Jv::s("ant-ling"),
            "reasoning" => Jv::b(reasoning),
            "input" => Jv::str_list(&["text"]),
            "cost" => Jv::Obj(obj! {
                "input" => Jv::n(cost[0]),
                "output" => Jv::n(cost[1]),
                "cacheRead" => Jv::n(cost[2]),
                "cacheWrite" => Jv::n(cost[3]),
            }),
            "contextWindow" => Jv::n(262_144.0),
            "maxTokens" => Jv::n(65_536.0),
            "compat" => Jv::Obj(compat),
        }
    };
    models.push(ant_ling_entry(
        "Ling-2.6-flash",
        "Ling 2.6 Flash",
        false,
        [0.01, 0.02, 0.0, 0.0],
        ant_ling_compat.clone(),
    ));
    models.push(ant_ling_entry(
        "Ling-2.6-1T",
        "Ling 2.6 1T",
        false,
        [0.06, 0.25, 0.0, 0.0],
        ant_ling_compat.clone(),
    ));
    let mut ring_compat = ant_ling_compat.clone();
    ring_compat.set("thinkingFormat", Jv::s("ant-ling"));
    models.push(ant_ling_entry(
        "Ring-2.6-1T",
        "Ring 2.6 1T",
        true,
        [0.06, 0.25, 0.0, 0.0],
        ring_compat,
    ));

    for candidate in models.iter_mut() {
        if string_value(candidate, "api") == Some("openai-completions")
            && string_value(candidate, "id").is_some_and(|id| id.contains("deepseek-v4"))
            && !string_value(candidate, "provider")
                .map(|provider| set_contains(QWEN_TOKEN_PLAN_PROVIDER_IDS, provider))
                .unwrap_or(false)
        {
            let preserves_native_reasoning_effort = matches!(
                string_value(candidate, "provider"),
                Some("openrouter") | Some("opencode")
            );
            let compat = if preserves_native_reasoning_effort {
                obj! { "requiresReasoningContentOnAssistantMessages" => Jv::b(true) }
            } else {
                deepseek_compat.clone()
            };
            merge_compat(candidate, compat);
        }
    }

    // MiniMax's Anthropic-compatible endpoints only serve these ids directly.
    models.retain(|candidate| {
        let provider = string_value(candidate, "provider").unwrap_or_default();
        let id = string_value(candidate, "id").unwrap_or_default();
        !(provider == "minimax" || provider == "minimax-cn")
            || set_contains(MINIMAX_DIRECT_SUPPORTED_IDS, id)
    });

    // OpenAI Codex (ChatGPT OAuth) models: not fetched from models.dev; a
    // small, explicit list avoids aliases. Older model limits are based on
    // observed server behavior; GPT-5.6 and GPT-6 Astra use Codex's 272k
    // default catalog limit.
    let codex_models: Vec<JsObj> = vec![
        static_codex_entry(StaticEntry {
            id: "gpt-6-astra",
            name: "GPT-6 Astra",
            api: "openai-codex-responses",
            base_url: "https://chatgpt.com/backend-api",
            provider: "openai-codex",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: long_context_cost(10.0, 50.0, 1.0, 12.5),
            context_window: 272_000.0,
            max_tokens: 128_000.0,
        }),
        static_codex_entry(StaticEntry {
            id: "gpt-5.3-codex-spark",
            name: "GPT-5.3 Codex Spark",
            api: "openai-codex-responses",
            base_url: "https://chatgpt.com/backend-api",
            provider: "openai-codex",
            reasoning: true,
            input: Jv::str_list(&["text"]),
            cost: obj! {
                "input" => Jv::n(1.75),
                "output" => Jv::n(14.0),
                "cacheRead" => Jv::n(0.175),
                "cacheWrite" => Jv::n(0.0),
            },
            context_window: 128_000.0,
            max_tokens: 128_000.0,
        }),
        static_codex_entry(StaticEntry {
            id: "gpt-5.5",
            name: "GPT-5.5",
            api: "openai-codex-responses",
            base_url: "https://chatgpt.com/backend-api",
            provider: "openai-codex",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: long_context_cost(5.0, 30.0, 0.5, 0.0),
            context_window: 272_000.0,
            max_tokens: 128_000.0,
        }),
        static_codex_entry(StaticEntry {
            id: "gpt-5.6-luna",
            name: "GPT-5.6 Luna",
            api: "openai-codex-responses",
            base_url: "https://chatgpt.com/backend-api",
            provider: "openai-codex",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: long_context_cost(0.2, 1.2, 0.02, 0.25),
            context_window: 272_000.0,
            max_tokens: 128_000.0,
        }),
        static_codex_entry(StaticEntry {
            id: "gpt-5.6-sol",
            name: "GPT-5.6 Sol",
            api: "openai-codex-responses",
            base_url: "https://chatgpt.com/backend-api",
            provider: "openai-codex",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: long_context_cost(5.0, 30.0, 0.5, 6.25),
            context_window: 272_000.0,
            max_tokens: 128_000.0,
        }),
        static_codex_entry(StaticEntry {
            id: "gpt-5.6-terra",
            name: "GPT-5.6 Terra",
            api: "openai-codex-responses",
            base_url: "https://chatgpt.com/backend-api",
            provider: "openai-codex",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: long_context_cost(2.0, 12.0, 0.2, 2.5),
            context_window: 272_000.0,
            max_tokens: 128_000.0,
        }),
    ];
    models.extend(codex_models);

    // Add missing Mistral Medium 3.5 model until models.dev includes it
    if !models.iter().any(|candidate| {
        string_value(candidate, "provider") == Some("mistral")
            && string_value(candidate, "id") == Some("mistral-medium-3.5")
    }) {
        models.push(static_codex_entry(StaticEntry {
            id: "mistral-medium-3.5",
            name: "Mistral Medium 3.5",
            api: "mistral-conversations",
            base_url: "https://api.mistral.ai",
            provider: "mistral",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: obj! {
                "input" => Jv::n(1.5),
                "output" => Jv::n(7.5),
                "cacheRead" => Jv::n(0.0),
                "cacheWrite" => Jv::n(0.0),
            },
            context_window: 262_144.0,
            max_tokens: // 256k tokens
            262_144.0,
        }));
    }

    // Add "auto" alias for openrouter/auto
    if !models.iter().any(|candidate| {
        string_value(candidate, "provider") == Some("openrouter")
            && string_value(candidate, "id") == Some("auto")
    }) {
        models.push(static_codex_entry(StaticEntry {
            id: "auto",
            name: "Auto",
            api: "openai-completions",
            base_url: "https://openrouter.ai/api/v1",
            provider: "openrouter",
            reasoning: true,
            input: Jv::str_list(&["text", "image"]),
            cost: // we dont know about the costs because OpenRouter auto routes to
            // different models and then charges you for the underlying used
            // model
            obj! {
                "input" => Jv::n(0.0),
                "output" => Jv::n(0.0),
                "cacheRead" => Jv::n(0.0),
                "cacheWrite" => Jv::n(0.0),
            },
            context_window: 2_000_000.0,
            max_tokens: 30_000.0,
        }));
    }

    // Add "fusion" alias for openrouter/fusion. OpenRouter exposes Fusion as
    // a router alias/plugin entry point; its model metadata does not
    // advertise tools, but the alias resolves to a concrete model that can
    // invoke caller tools.
    if !models.iter().any(|candidate| {
        string_value(candidate, "provider") == Some("openrouter")
            && string_value(candidate, "id") == Some("openrouter/fusion")
    }) {
        models.push(static_codex_entry(StaticEntry {
            id: "openrouter/fusion",
            name: "OpenRouter: Fusion",
            api: "openai-completions",
            base_url: "https://openrouter.ai/api/v1",
            provider: "openrouter",
            reasoning: true,
            input: Jv::str_list(&["text"]),
            cost: obj! {
                "input" => Jv::n(0.0),
                "output" => Jv::n(0.0),
                "cacheRead" => Jv::n(0.0),
                "cacheWrite" => Jv::n(0.0),
            },
            context_window: 1_000_000.0,
            max_tokens: 30_000.0,
        }));
    }

    // Azure Foundry deploys these with larger context windows than OpenAI's
    // own short-tier defaults.
    let azure_context_window_overrides: &[(&str, f64)] = &[
        ("gpt-5.4", 1_050_000.0),
        ("gpt-5.5", 1_050_000.0),
        ("gpt-5.6-luna", 1_050_000.0),
        ("gpt-5.6-sol", 1_050_000.0),
        ("gpt-5.6-terra", 1_050_000.0),
    ];
    let azure_clones: Vec<JsObj> = models
        .iter()
        .filter(|model| {
            string_value(model, "provider") == Some("openai")
                && string_value(model, "api") == Some("openai-responses")
        })
        .map(|model| {
            let mut clone = model.clone();
            let id = string_value(&clone, "id").unwrap_or_default().to_string();
            clone.set("api", Jv::s("azure-openai-responses"));
            clone.set("provider", Jv::s("azure-openai-responses"));
            clone.set("baseUrl", Jv::s(""));
            let cost = obj! {
                "input" => Jv::n(cost_field(model, "input")),
                "output" => Jv::n(cost_field(model, "output")),
                "cacheRead" => Jv::n(cost_field(model, "cacheRead")),
                "cacheWrite" => Jv::n(cost_field(model, "cacheWrite")),
            };
            clone.set("cost", Jv::Obj(cost));
            let context_window = azure_context_window_overrides
                .iter()
                .find(|(key, _)| *key == id)
                .map(|(_, value)| *value)
                .unwrap_or_else(|| context_window_of(model));
            set_numeric(&mut clone, "contextWindow", context_window);
            clone
        })
        .collect();
    models.extend(azure_clones);
}

fn context_window_of(model: &JsObj) -> f64 {
    model
        .get("contextWindow")
        .and_then(Jv::as_f64)
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Catalog assembly (generate-models.ts:2550-2565, 3025-3087)
// ---------------------------------------------------------------------------

/// The four source documents (see `main.rs::fetch_sources`).
pub(crate) struct CatalogSources {
    pub(crate) models_dev: Json,
    pub(crate) openrouter: Json,
    pub(crate) ai_gateway: Json,
    pub(crate) nvidia_nim: Option<Json>,
}

/// One generated catalog run.
pub(crate) struct GeneratedCatalog {
    pub(crate) structure: ModelDataStructure,
    /// filename → serialized content, exactly the bytes written to disk.
    pub(crate) files: BTreeMap<String, String>,
    pub(crate) warnings: Vec<String>,
}

/// `generateModels` minus the .models.ts emission: fetch-derived loaders,
/// filters, overrides, additions, the apply-pipeline, then the grouped and
/// sorted per-provider serialization.
pub(crate) fn generate_catalog(sources: &CatalogSources) -> GeneratedCatalog {
    let mut warnings: Vec<String> = Vec::new();
    let mut state = LoaderState::new();
    let nim_model_ids = crate::providers::nvidia_nim_model_ids(sources.nvidia_nim.as_ref());
    load_models_dev_data(&sources.models_dev, &nim_model_ids, &mut state);
    fetch_openrouter_models(&sources.openrouter, &mut state);
    fetch_ai_gateway_models(&sources.ai_gateway, &mut state);
    let mut models: Vec<JsObj> = state.models;
    let recorded = state.recorded;

    // Combine models (models.dev has priority over OpenRouter, then the
    // Vercel AI Gateway).
    models.retain(|model| {
        let provider = string_value(model, "provider").unwrap_or_default();
        let id = string_value(model, "id").unwrap_or_default();
        !(provider == "xai" && set_contains(XAI_BUILTIN_EXCLUDED_MODEL_IDS, id))
            && !((provider == "opencode" || provider == "opencode-go")
                && id == "gpt-5.3-codex-spark")
    });

    apply_model_overrides(&mut models);
    apply_additive_passes(&mut models);

    for model in models.iter_mut() {
        apply_openai_completions_compat_metadata(model);
        apply_anthropic_messages_compat_metadata(model);
        apply_models_dev_reasoning_option_metadata(model, &recorded);
        apply_thinking_level_metadata(model, &recorded);
        apply_strict_tool_compat_metadata(model);
        apply_openai_grammar_tool_compat_metadata(model);
        apply_openai_tool_search_metadata(model);
        apply_openai_completions_transcript_metadata(model);
        apply_openai_responses_transcript_metadata(model);
        apply_openai_explicit_prompt_cache_metadata(model);
    }
    apply_anthropic_allowed_fallback_metadata(&mut models);

    // Group by provider and deduplicate by model id (first wins, so
    // models.dev keeps priority over the secondary sources).
    let mut providers: BTreeMap<String, BTreeMap<String, JsObj>> = BTreeMap::new();
    for model in models.iter() {
        let provider = string_value(model, "provider")
            .unwrap_or_default()
            .to_string();
        let id = string_value(model, "id").unwrap_or_default().to_string();
        providers
            .entry(provider)
            .or_default()
            .entry(id)
            .or_insert_with(|| model.clone());
    }

    // Only the ignored internal data is grouped by API for type derivation;
    // the data files group each provider by API with sorted keys.
    let mut structure: ModelDataStructure = BTreeMap::new();
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    for (provider_id, provider_models) in &providers {
        let mut structure_models: BTreeMap<String, String> = BTreeMap::new();
        let mut api_groups: BTreeMap<&str, BTreeMap<&str, &JsObj>> = BTreeMap::new();
        for (model_id, model) in provider_models {
            let api = string_value(model, "api").unwrap_or_default();
            structure_models.insert(model_id.clone(), api.to_string());
            api_groups.entry(api).or_default().insert(model_id, model);
        }
        let document = JsObj::from_pairs(
            api_groups
                .into_iter()
                .map(|(api, group)| {
                    (
                        api,
                        Jv::Obj(JsObj::from_pairs(
                            group
                                .into_iter()
                                .map(|(model_id, model)| {
                                    (model_id.to_string(), Jv::Obj(model.clone()))
                                })
                                .collect(),
                        )),
                    )
                })
                .collect(),
        );
        files.insert(
            format!("{provider_id}.json"),
            crate::json::serialize_json(&Jv::Obj(document)),
        );
        structure.insert(provider_id.clone(), structure_models);
    }

    if providers.is_empty() {
        warnings.push("no models were generated from the live sources".to_string());
    }

    GeneratedCatalog {
        structure,
        files,
        warnings,
    }
}

/// Reduce a full run to the given provider ids (files, structure, and the
/// derived manifest must all be rebuilt from the same subset).
pub(crate) fn restrict_catalog(catalog: &mut GeneratedCatalog, providers: &[String]) {
    catalog
        .structure
        .retain(|id, _| providers.iter().any(|provider| provider == id));
    catalog.files.retain(|file, _| {
        providers
            .iter()
            .any(|provider| format!("{provider}.json") == *file)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;

    /// The committed snapshot shards, the byte-level oracle for the fixture
    /// run (upstream's own generator output for the same source entries).
    fn committed_shard(provider: &str) -> Json {
        let path: &'static str = match provider {
            "amazon-bedrock" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/amazon-bedrock.json"
            )),
            "anthropic" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/anthropic.json"
            )),
            "azure-openai-responses" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/azure-openai-responses.json"
            )),
            "google" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/google.json"
            )),
            "google-vertex" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/google-vertex.json"
            )),
            "github-copilot" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/github-copilot.json"
            )),
            "groq" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/groq.json"
            )),
            "mistral" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/mistral.json"
            )),
            "moonshotai" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/moonshotai.json"
            )),
            "opencode" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/opencode.json"
            )),
            "opencode-go" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/opencode-go.json"
            )),
            "openrouter" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/openrouter.json"
            )),
            "qwen-token-plan" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/qwen-token-plan.json"
            )),
            "together" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/together.json"
            )),
            "vercel-ai-gateway" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/vercel-ai-gateway.json"
            )),
            "cloudflare-workers-ai" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/cloudflare-workers-ai.json"
            )),
            "baseten" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/baseten.json"
            )),
            "fireworks" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/fireworks.json"
            )),
            "zai" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/zai.json"
            )),
            "zai-coding-cn" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/zai-coding-cn.json"
            )),
            "openai" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/model-data/openai.json"
            )),
            other => panic!("no committed shard mapping for {other}"),
        };
        serde_json::from_str(path).expect("committed shard parses")
    }

    fn fixture_file(name: &str) -> String {
        let mut path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/generate-models/"
        )
        .to_string();
        path.push_str(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read fixture {name}: {error}"))
    }

    fn fixture_sources() -> CatalogSources {
        CatalogSources {
            models_dev: serde_json::from_str(&fixture_file("models-dev.json"))
                .expect("fixture models.dev parses"),
            openrouter: serde_json::from_str(&fixture_file("openrouter.json"))
                .expect("fixture openrouter parses"),
            ai_gateway: serde_json::from_str(&fixture_file("ai-gateway.json"))
                .expect("fixture gateway parses"),
            nvidia_nim: Some(
                serde_json::from_str(&fixture_file("nvidia-nim.json")).expect("fixture NIM parses"),
            ),
        }
    }

    fn fixture_catalog() -> GeneratedCatalog {
        generate_catalog(&fixture_sources())
    }

    /// The generated `{api: {modelId: model}}` document for one provider,
    /// re-parsed as unordered JSON for value comparisons.
    fn generated_document(catalog: &GeneratedCatalog, provider: &str) -> Json {
        let content = catalog
            .files
            .get(&format!("{provider}.json"))
            .unwrap_or_else(|| panic!("provider {provider} was not generated"));
        serde_json::from_str(content).expect("generated document parses")
    }

    fn document_model<'a>(document: &'a Json, api: &str, model_id: &str) -> &'a Json {
        document
            .get(api)
            .and_then(|group| group.get(model_id))
            .unwrap_or_else(|| panic!("model {api}/{model_id} missing"))
    }

    fn assert_matches_committed(provider: &str, api: &str, model_id: &str) {
        let catalog = fixture_catalog();
        let generated = generated_document(&catalog, provider);
        let committed = committed_shard(provider);
        assert_eq!(
            document_model(&generated, api, model_id),
            document_model(&committed, api, model_id),
            "{provider}/{model_id} must match the committed snapshot value"
        );
    }

    #[test]
    fn fixture_generates_the_expected_provider_set() {
        let catalog = fixture_catalog();
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
            "together",
            "vercel-ai-gateway",
            "xai",
            "xiaomi",
            "xiaomi-token-plan-cn",
            "zai",
            "zai-coding-cn",
        ];
        let providers: Vec<&str> = catalog.structure.keys().map(String::as_str).collect();
        assert_eq!(providers, expected);
        // kimi-coding stays absent: live models.dev has no kimi-for-coding
        // section since the snapshot (the disclosed drift).
        assert!(!catalog.structure.contains_key("kimi-coding"));
        // radius is a dynamic provider with no static catalog.
        assert!(!catalog.structure.contains_key("radius"));
    }

    /// Providers whose fixture input is exactly the committed model set (or
    /// fully static content) must reproduce the committed file byte for byte.
    #[test]
    fn static_and_untrimmed_providers_are_byte_identical_to_the_snapshot() {
        let catalog = fixture_catalog();
        let committed: &[(&str, &str)] = &[
            (
                "ant-ling",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/assets/model-data/ant-ling.json"
                )),
            ),
            (
                "deepseek",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/assets/model-data/deepseek.json"
                )),
            ),
            (
                "openai-codex",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/assets/model-data/openai-codex.json"
                )),
            ),
            (
                "xai",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/assets/model-data/xai.json"
                )),
            ),
        ];
        for (provider, committed_bytes) in committed {
            let generated = catalog
                .files
                .get(&format!("{provider}.json"))
                .unwrap_or_else(|| panic!("{provider} was generated"));
            assert_eq!(
                generated, committed_bytes,
                "{provider}.json must be byte-identical"
            );
        }
    }

    #[test]
    fn generated_models_match_the_committed_snapshot_values() {
        // One representative per transformation path; the committed entries
        // are upstream's generator output for the same live source entries.
        assert_matches_committed("anthropic", "anthropic-messages", "claude-fable-5");
        assert_matches_committed("openai", "openai-responses", "gpt-5.5");
        assert_matches_committed("openai", "openai-responses", "gpt-5-pro");
        assert_matches_committed("google", "google-generative-ai", "gemini-flash-latest");
        assert_matches_committed("google-vertex", "google-vertex", "gemini-2.5-flash");
        assert_matches_committed("github-copilot", "anthropic-messages", "claude-opus-4.7");
        assert_matches_committed("github-copilot", "openai-responses", "gpt-5.5");
        assert_matches_committed("together", "openai-completions", "MiniMaxAI/MiniMax-M2.7");
        assert_matches_committed(
            "together",
            "openai-completions",
            "Qwen/Qwen2.5-7B-Instruct-Turbo",
        );
        assert_matches_committed("baseten", "openai-completions", "zai-org/GLM-5.2");
        assert_matches_committed(
            "fireworks",
            "openai-completions",
            "accounts/fireworks/models/kimi-k3",
        );
        assert_matches_committed(
            "fireworks",
            "anthropic-messages",
            "accounts/fireworks/models/qwen3p8-max",
        );
        assert_matches_committed("opencode", "openai-completions", "kimi-k2.6");
        assert_matches_committed("opencode-go", "openai-completions", "minimax-m2.7");
        assert_matches_committed("opencode-go", "openai-completions", "qwen3.6-plus");
        assert_matches_committed("moonshotai", "openai-completions", "kimi-k3");
        assert_matches_committed("moonshotai", "openai-completions", "kimi-k2.7-code");
        assert_matches_committed("zai", "openai-completions", "glm-5.2");
        // Live zhipuai-coding-plan drifted past the snapshot (glm-5.2 ->
        // glm-5.3-highspeed), so only presence is pinned here.
        let zai_cn = generated_document(&fixture_catalog(), "zai-coding-cn");
        assert!(zai_cn["openai-completions"]
            .get("glm-5.3-highspeed")
            .is_some());
        assert_matches_committed("mistral", "mistral-conversations", "mistral-small-2506");
        assert_matches_committed("groq", "openai-completions", "qwen/qwen3.6-27b");
        assert_matches_committed(
            "amazon-bedrock",
            "bedrock-converse-stream",
            "eu.amazon.nova-2-lite-v1:0",
        );
        assert_matches_committed(
            "vercel-ai-gateway",
            "anthropic-messages",
            "moonshotai/kimi-k3",
        );
        assert_matches_committed(
            "openrouter",
            "anthropic-messages",
            "anthropic/claude-3-haiku",
        );
        assert_matches_committed("openrouter", "openai-completions", "openai/gpt-5.5");
        assert_matches_committed(
            "cloudflare-workers-ai",
            "openai-completions",
            "@cf/meta/llama-3.3-70b-instruct-fp8-fast",
        );
        assert_matches_committed("qwen-token-plan", "openai-completions", "glm-5");
    }

    #[test]
    fn live_drift_and_filters_shape_the_provider_sets() {
        let catalog = fixture_catalog();

        // The qwen token plan excluded preview id and the Individual
        // allowlist.
        assert!(!catalog.structure["qwen-token-plan"].contains_key("qwen3.8-max-preview"));
        assert!(catalog.structure["qwen-token-plan"].contains_key("qwen3.8-max"));
        let individual: Vec<&str> = catalog.structure["qwen-token-plan-individual"]
            .keys()
            .map(String::as_str)
            .collect();
        // The allowlist intersects the fixture source in exactly these ids
        // (glm-5 is in the fallback map, not the allowlist).
        assert_eq!(individual, ["qwen3.6-flash", "qwen3.8-max"]);

        // MiniMax's Anthropic endpoints only serve the direct ids.
        let minimax: Vec<&str> = catalog.structure["minimax"]
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(minimax, ["MiniMax-M2.7", "MiniMax-M3"]);

        // NVIDIA NIM liveness: only ids present in the live NIM list, minus
        // the unsupported set.
        let nvidia: Vec<&str> = catalog.structure["nvidia"]
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(nvidia, ["qwen/qwen3-coder-480b-a35b-instruct"]);

        // Bedrock: the inference-profile-only id is dropped; eu.* models keep
        // the eu endpoint and sort first.
        let bedrock: Vec<&str> = catalog.structure["amazon-bedrock"]
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            bedrock,
            [
                "amazon.nova-2-lite-v1:0",
                "anthropic.claude-sonnet-4-5-20250929-v1:0",
                "eu.amazon.nova-2-lite-v1:0",
            ]
        );
        assert!(!catalog.structure["amazon-bedrock"].contains_key("anthropic.claude-opus-5"));

        // The opencode gpt-5.3-codex-spark filter applies.
        assert!(!catalog.structure["opencode"].contains_key("gpt-5.3-codex-spark"));

        // The non-tool OpenRouter entry is dropped.
        assert!(!catalog.structure["openrouter"].contains_key("inference-net/schematron-v2-turbo"));
        // The openrouter auto alias is synthesized.
        assert!(catalog.structure["openrouter"].contains_key("auto"));
    }

    #[test]
    fn static_addition_and_override_paths_match_the_snapshot() {
        let catalog = fixture_catalog();
        // Long-context tiers + the Azure context override flow into the
        // cloned entries.
        let azure = generated_document(&catalog, "azure-openai-responses");
        let luna = document_model(&azure, "azure-openai-responses", "gpt-5.6-luna");
        assert_eq!(luna["contextWindow"], 1_050_000);
        // The azure clone rebuilds the cost object (no tiers), like the
        // upstream literal spread.
        assert!(luna["cost"].get("tiers").is_none());
        let openai = generated_document(&catalog, "openai");
        assert_eq!(
            document_model(&openai, "openai-responses", "gpt-5.6-luna")["cost"]["tiers"][0]
                ["inputTokensAbove"],
            272_000
        );
        // Codex list keeps its catalog limits.
        let codex = generated_document(&catalog, "openai-codex");
        assert_eq!(
            document_model(&codex, "openai-codex-responses", "gpt-5.6-luna")["contextWindow"],
            272_000
        );
        // The static deepseek entries keep the flash map on flash and get the
        // standard map from the deepseek-v4 loop on pro.
        let deepseek = generated_document(&catalog, "deepseek");
        assert_eq!(
            document_model(&deepseek, "openai-completions", "deepseek-flash")["thinkingLevelMap"]
                ["low"],
            "low"
        );
        assert_eq!(
            document_model(&deepseek, "openai-completions", "deepseek-v4-pro")["thinkingLevelMap"]
                ["low"],
            Json::Null
        );
    }

    #[test]
    fn openrouter_batch_keeps_the_upstream_compat_key_order() {
        let catalog = fixture_catalog();
        let content = &catalog.files["openrouter.json"];
        // The delta order places cacheControlFormat before
        // sendSessionAffinityHeaders (upstream return-literal order).
        let marker = "\"compat\":{\"thinkingFormat\":\"openrouter\",\"cacheControlFormat\":\"anthropic\",\"sendSessionAffinityHeaders\":true}";
        assert!(content.contains(marker), "batch compat order missing");
    }

    #[test]
    fn api_groups_serialize_sorted() {
        let catalog = fixture_catalog();
        let copilot = catalog.files["github-copilot.json"].clone();
        let anthropic_at = copilot
            .find("\"anthropic-messages\"")
            .expect("copilot anthropic group");
        let completions_at = copilot
            .find("\"openai-completions\"")
            .expect("copilot completions group");
        let responses_at = copilot
            .find("\"openai-responses\"")
            .expect("copilot responses group");
        assert!(anthropic_at < completions_at && completions_at < responses_at);
    }

    #[test]
    fn every_generated_model_deserializes_into_the_catalog_model() {
        let catalog = fixture_catalog();
        for (filename, content) in &catalog.files {
            let document: Json = serde_json::from_str(content).expect("generated document parses");
            for (api, group) in document.as_object().expect("api groups") {
                for (model_id, model) in group.as_object().expect("model group") {
                    let model: pi_rust::ai::types::Model = serde_json::from_value(model.clone())
                        .unwrap_or_else(|error| panic!("{filename} {api}/{model_id}: {error}"));
                    assert_eq!(model.id, *model_id);
                    assert_eq!(model.provider, filename.trim_end_matches(".json"));
                    assert_eq!(model.api, *api);
                }
            }
        }
    }

    #[test]
    fn structure_and_manifest_semantics_match_catalog_validation() {
        let catalog = fixture_catalog();
        let manifest: Json = serde_json::from_str(&crate::build_manifest(
            &catalog.structure,
            &catalog.files,
            "2026-09-21T02:08:23.378Z",
        ))
        .expect("manifest parses");
        assert_eq!(manifest["schemaVersion"], 3);
        assert_eq!(
            manifest["structureHash"],
            pi_rust::ai::models::catalog::model_data_structure_hash(&catalog.structure)
        );
        // The api structure derived from the files matches the structure map.
        for (provider_id, models) in &catalog.structure {
            let document: Json =
                serde_json::from_str(&catalog.files[&format!("{provider_id}.json")]).unwrap();
            for (api, group) in document.as_object().unwrap() {
                for model_id in group.as_object().unwrap().keys() {
                    assert_eq!(models[model_id], *api, "{provider_id}/{model_id}");
                }
            }
        }
    }

    /// The kimi-for-coding section is currently absent from live models.dev
    /// (disclosed drift); a synthetic section pins the loader for the day it
    /// returns.
    #[test]
    fn synthetic_kimi_coding_section_produces_the_provider() {
        let mut sources = fixture_sources();
        sources.models_dev.as_object_mut().unwrap().insert(
            "kimi-for-coding".to_string(),
            serde_json::json!({
                "models": {
                    "k3": {
                        "id": "k3", "name": "Kimi K3", "tool_call": true, "reasoning": true,
                        "limit": {"context": 262144, "output": 131072}
                    },
                    "kimi-for-coding": {
                        "id": "kimi-for-coding", "name": "Kimi For Coding", "tool_call": true,
                        "limit": {"context": 262144, "output": 131072}
                    },
                    "k2p5": {
                        "id": "k2p5", "name": "Kimi For Coding k2p5", "tool_call": true,
                        "limit": {"context": 262144, "output": 131072}
                    }
                }
            }),
        );
        let catalog = generate_catalog(&sources);
        let kimi = generated_document(&catalog, "kimi-coding");
        // Alias normalized to the canonical id and dropped as a duplicate.
        assert_eq!(
            kimi["anthropic-messages"]
                .as_object()
                .unwrap()
                .keys()
                .count(),
            2
        );
        let k3 = document_model(&kimi, "anthropic-messages", "k3");
        assert_eq!(k3["cost"]["input"], 3);
        assert_eq!(k3["compat"]["forceAdaptiveThinking"], true);
        assert_eq!(k3["compat"]["allowEmptySignature"], true);
        assert_eq!(k3["reasoning"], true);
        let canonical = document_model(&kimi, "anthropic-messages", "kimi-for-coding");
        assert_eq!(canonical["cost"]["input"], 0.95);
        assert_eq!(canonical["reasoning"], false);
    }
}
