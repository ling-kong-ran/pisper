//! Port of the source loaders in `scripts/generate-models.ts`:
//! `loadModelsDevData` (generate-models.ts:1658-2548), `fetchOpenRouterModels`
//! (1208-1271), and `fetchAiGatewayModels` (1273-1332). Each loop mirrors the
//! upstream literal construction order so the emitted objects serialize
//! byte-identically; the apply-pipeline lives in `transform.rs`.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value as Json;

use crate::json::{js_or, js_parse_f64, obj, object, round_cost, JsObj, Jv};
use crate::reasoning_options::{record_reasoning_options, ReasoningOption};
use crate::transform::{
    get_anthropic_messages_compat, get_models_dev_cost, google_thinking_level_map, level_map,
    moonshot_compat, normalize_nvidia_model_id, nvidia_openai_compat, qwen_token_plan_compat,
    set_contains, xai_responses_compat, xiaomi_compat, BEDROCK_INFERENCE_PROFILE_ONLY_MODEL_IDS,
    CLOUDFLARE_AI_GATEWAY_ANTHROPIC_BASE_URL, CLOUDFLARE_AI_GATEWAY_COMPAT_BASE_URL,
    CLOUDFLARE_AI_GATEWAY_OPENAI_BASE_URL, CLOUDFLARE_WORKERS_AI_BASE_URL, COPILOT_STATIC_HEADERS,
    FIREWORKS_ADAPTIVE_THINKING_FALLBACK_MODELS, KIMI_ALIASES, KIMI_CODING_IMPLIED_COSTS,
    MODELS_DEV_OPENAI_UNSUPPORTED_MODEL_IDS, NVIDIA_BASE_URL, NVIDIA_HEADERS,
    NVIDIA_NIM_UNSUPPORTED_MODELS, OPENCODE_LONG_CACHE_RETENTION_UNSUPPORTED_MODELS,
    QWEN_TOKEN_PLAN_EXCLUDED_MODEL_IDS, QWEN_TOKEN_PLAN_FALLBACK_THINKING_LEVEL_MAP,
    QWEN_TOKEN_PLAN_INDIVIDUAL_MODEL_IDS, QWEN_TOKEN_PLAN_REASONING_EFFORT_FALLBACK_MODEL_IDS,
    TOGETHER_BASE_URL, TOGETHER_DEEPSEEK_V4_THINKING_LEVEL_MAP, TOGETHER_FIXED_REASONING_LEVEL_MAP,
    TOGETHER_REASONING_EFFORT_LEVEL_MAP, TOGETHER_REASONING_EFFORT_MODELS,
    TOGETHER_REASONING_ONLY_MODELS, TOGETHER_TOGGLE_REASONING_EFFORT_MODELS,
    TOGETHER_TOGGLE_REASONING_LEVEL_MAP, VERTEX_BASE_URL, ZAI_TOOL_STREAM_UNSUPPORTED_MODELS,
};

/// A model under construction: the ordered object the upstream literals build.
pub(crate) type Model = JsObj;

/// The per-run state the loaders record into (upstream's
/// `modelsDevReasoningOptions` map plus the flat model list).
pub(crate) struct LoaderState {
    pub(crate) models: Vec<Model>,
    pub(crate) recorded: HashMap<String, Vec<ReasoningOption>>,
}

impl LoaderState {
    pub(crate) fn new() -> LoaderState {
        LoaderState {
            models: Vec::new(),
            recorded: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// models.dev document accessors
// ---------------------------------------------------------------------------

fn doc_str<'a>(value: &'a Json, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Json::as_str)
}

fn doc_bool(value: &Json, key: &str) -> Option<bool> {
    value.get(key).and_then(Json::as_bool)
}

fn doc_f64(value: &Json, key: &str) -> Option<f64> {
    value.get(key).and_then(Json::as_f64)
}

/// `m.limit?.context || 4096` and friends (`||` collapses 0/nullish).
fn limit_or(model: &Json, key: &str, fallback: f64) -> f64 {
    let raw = model
        .get("limit")
        .and_then(|limit| limit.get(key))
        .and_then(Json::as_f64);
    js_or(raw, fallback)
}

/// `m.modalities?.input?.includes("image")`.
fn modalities_include_image(model: &Json) -> bool {
    model
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
        .and_then(Json::as_array)
        .is_some_and(|input| input.iter().any(|entry| entry.as_str() == Some("image")))
}

/// `m.modalities?.input?.includes("image") ? ["text", "image"] : ["text"]`.
fn input_modalities(model: &Json) -> Jv {
    if modalities_include_image(model) {
        Jv::str_list(&["text", "image"])
    } else {
        Jv::str_list(&["text"])
    }
}

/// `{input: m.cost?.input || 0, ...}` — the plain four-field cost literal.
fn source_cost(model: &Json) -> JsObj {
    let cost = model.get("cost");
    obj! {
        "input" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "input")), 0.0)),
        "output" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "output")), 0.0)),
        "cacheRead" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "cache_read")), 0.0)),
        "cacheWrite" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "cache_write")), 0.0)),
    }
}

/// The provider `models` map from the models.dev document.
fn section_models<'a>(
    document: &'a Json,
    section: &str,
) -> Option<&'a serde_json::Map<String, Json>> {
    document
        .get(section)
        .and_then(|section| section.get("models"))
        .and_then(Json::as_object)
}

// ---------------------------------------------------------------------------
// models.dev loader (generate-models.ts:1658-2548), in upstream call order
// ---------------------------------------------------------------------------

pub(crate) fn load_models_dev_data(
    document: &Json,
    nim_model_ids: &HashMap<String, String>,
    state: &mut LoaderState,
) {
    let mut cloudflare_ai_gateway_model_ids: BTreeSet<String> = BTreeSet::new();

    // Process Amazon Bedrock models
    if let Some(models) = section_models(document, "amazon-bedrock") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            if set_contains(BEDROCK_INFERENCE_PROFILE_ONLY_MODEL_IDS, model_id) {
                continue;
            }
            if model_id.starts_with("ai21.jamba") {
                // These models don't support tool use in streaming mode
                continue;
            }
            if model_id.starts_with("mistral.mistral-7b-instruct-v0") {
                // These models don't support system messages
                continue;
            }
            let mut entry = object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("bedrock-converse-stream"),
                "provider" => Jv::s("amazon-bedrock"),
                "baseUrl" => Jv::s(bedrock_base_url(model_id)),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            };
            if doc_bool(model, "structured_output") == Some(true) {
                entry.set(
                    "compat",
                    Jv::Obj(obj! { "supportsStrictMode" => Jv::b(true) }),
                );
            }
            state.models.push(entry);
            record_reasoning_options(&mut state.recorded, "amazon-bedrock", model_id, model);
        }
    }

    // Process Anthropic models
    if let Some(models) = section_models(document, "anthropic") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            state.models.push(object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("anthropic-messages"),
                "provider" => Jv::s("anthropic"),
                "baseUrl" => Jv::s("https://api.anthropic.com"),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            });
            record_reasoning_options(&mut state.recorded, "anthropic", model_id, model);
        }
    }

    process_google_models(document, state);

    // Process OpenAI models
    if let Some(models) = section_models(document, "openai") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            // models.dev lists this alias, but it is not accepted by OpenAI APIs.
            if set_contains(MODELS_DEV_OPENAI_UNSUPPORTED_MODEL_IDS, model_id) {
                continue;
            }
            state.models.push(object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("openai-responses"),
                "provider" => Jv::s("openai"),
                "baseUrl" => Jv::s("https://api.openai.com/v1"),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            });
            record_reasoning_options(&mut state.recorded, "openai", model_id, model);
        }
    }

    // Process Groq models
    if let Some(models) = section_models(document, "groq") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            state.models.push(object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("openai-completions"),
                "provider" => Jv::s("groq"),
                "baseUrl" => Jv::s("https://api.groq.com/openai/v1"),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            });
            record_reasoning_options(&mut state.recorded, "groq", model_id, model);
        }
    }

    // Process Cerebras models
    if let Some(models) = section_models(document, "cerebras") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            state.models.push(object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("openai-completions"),
                "provider" => Jv::s("cerebras"),
                "baseUrl" => Jv::s("https://api.cerebras.ai/v1"),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            });
            record_reasoning_options(&mut state.recorded, "cerebras", model_id, model);
        }
    }

    // Process Cloudflare Workers AI models
    if let Some(models) = section_models(document, "cloudflare-workers-ai") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            state.models.push(object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("openai-completions"),
                "provider" => Jv::s("cloudflare-workers-ai"),
                "baseUrl" => Jv::s(CLOUDFLARE_WORKERS_AI_BASE_URL),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
                "compat" => Jv::Obj(obj! { "sendSessionAffinityHeaders" => Jv::b(true) }),
            });
            record_reasoning_options(
                &mut state.recorded,
                "cloudflare-workers-ai",
                model_id,
                model,
            );
        }
    }

    // Process Cloudflare AI Gateway models
    if let Some(models) = section_models(document, "cloudflare-ai-gateway") {
        for (prefixed_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            let Some((upstream, native_id)) = prefixed_id.split_once('/') else {
                continue;
            };

            let (api, base_url, id): (&str, &str, &str) = match upstream {
                "openai" => (
                    "openai-responses",
                    CLOUDFLARE_AI_GATEWAY_OPENAI_BASE_URL,
                    native_id,
                ),
                "anthropic" => (
                    "anthropic-messages",
                    CLOUDFLARE_AI_GATEWAY_ANTHROPIC_BASE_URL,
                    native_id,
                ),
                "workers-ai" => (
                    "openai-completions",
                    CLOUDFLARE_AI_GATEWAY_COMPAT_BASE_URL,
                    prefixed_id,
                ),
                _ => continue,
            };

            // Gateway passthroughs forward session affinity headers to
            // upstreams that use them for cache/routing affinity.
            let compat = (upstream == "anthropic" || upstream == "workers-ai")
                .then(|| obj! { "sendSessionAffinityHeaders" => Jv::b(true) });

            cloudflare_ai_gateway_model_ids.insert(id.to_string());
            let mut entry = object! {
                "id" => Jv::s(id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(id)),
                "api" => Jv::s(api),
                "provider" => Jv::s("cloudflare-ai-gateway"),
                "baseUrl" => Jv::s(base_url),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            };
            if let Some(compat) = compat {
                entry.set("compat", Jv::Obj(compat));
            }
            state.models.push(entry);
            record_reasoning_options(&mut state.recorded, "cloudflare-ai-gateway", id, model);
        }
    }

    // The gateway proxies Workers AI through its OpenAI-compatible /compat
    // endpoint, but models.dev may omit or intermittently drop those
    // `workers-ai/*` entries from the AI Gateway catalog. Mirror the Workers
    // AI catalog under the documented prefix so the gateway keeps its
    // OpenAI-compatible models stable.
    if let Some(models) = section_models(document, "cloudflare-workers-ai") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            let id = format!("workers-ai/{model_id}");
            if !cloudflare_ai_gateway_model_ids.insert(id.clone()) {
                continue;
            }
            state.models.push(object! {
                "id" => Jv::s(&id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(&id)),
                "api" => Jv::s("openai-completions"),
                "provider" => Jv::s("cloudflare-ai-gateway"),
                "baseUrl" => Jv::s(CLOUDFLARE_AI_GATEWAY_COMPAT_BASE_URL),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
                "compat" => Jv::Obj(obj! { "sendSessionAffinityHeaders" => Jv::b(true) }),
            });
            record_reasoning_options(&mut state.recorded, "cloudflare-ai-gateway", &id, model);
        }
    }

    // Process xAi models
    if let Some(models) = section_models(document, "xai") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            state.models.push(object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("openai-responses"),
                "provider" => Jv::s("xai"),
                "baseUrl" => Jv::s("https://api.x.ai/v1"),
                "compat" => Jv::Obj(xai_responses_compat()),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            });
            record_reasoning_options(&mut state.recorded, "xai", model_id, model);
        }
    }

    process_zai_models(document, state);

    // Process Mistral models
    if let Some(models) = section_models(document, "mistral") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            let cost = model.get("cost");
            let cache_read = match cost
                .and_then(|cost| cost.get("cache_read"))
                .and_then(Json::as_f64)
            {
                // `??` only: a present 0 stays 0.
                Some(value) => value,
                None => match cost.and_then(|cost| doc_f64(cost, "input")) {
                    Some(input) if input != 0.0 => round_cost(input * 0.1),
                    _ => 0.0,
                },
            };
            state.models.push(object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("mistral-conversations"),
                "provider" => Jv::s("mistral"),
                "baseUrl" => Jv::s("https://api.mistral.ai"),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(obj! {
                    "input" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "input")), 0.0)),
                    "output" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "output")), 0.0)),
                    "cacheRead" => Jv::n(cache_read),
                    "cacheWrite" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "cache_write")), 0.0)),
                }),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            });
            record_reasoning_options(&mut state.recorded, "mistral", model_id, model);
        }
    }

    // Process Hugging Face models
    if let Some(models) = section_models(document, "huggingface") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            state.models.push(object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("openai-completions"),
                "provider" => Jv::s("huggingface"),
                "baseUrl" => Jv::s("https://router.huggingface.co/v1"),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "compat" => Jv::Obj(obj! { "supportsDeveloperRole" => Jv::b(false) }),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            });
            record_reasoning_options(&mut state.recorded, "huggingface", model_id, model);
        }
    }

    process_fireworks_models(section_models(document, "fireworks-ai"), state);

    // Process NVIDIA NIM models
    if let Some(models) = section_models(document, "nvidia") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            let has_text = |key: &str| {
                model
                    .get("modalities")
                    .and_then(|modalities| modalities.get(key))
                    .and_then(Json::as_array)
                    .is_some_and(|values| values.iter().any(|entry| entry.as_str() == Some("text")))
            };
            if !has_text("input") || !has_text("output") {
                continue;
            }

            let Some(live_model_id) = nim_model_ids.get(model_id).cloned().or_else(|| {
                nim_model_ids
                    .get(&normalize_nvidia_model_id(model_id))
                    .cloned()
            }) else {
                continue;
            };
            if set_contains(NVIDIA_NIM_UNSUPPORTED_MODELS, &live_model_id) {
                continue;
            }

            state.models.push(object! {
                "id" => Jv::s(&live_model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(&live_model_id)),
                "api" => Jv::s("openai-completions"),
                "provider" => Jv::s("nvidia"),
                "baseUrl" => Jv::s(NVIDIA_BASE_URL),
                "headers" => Jv::Obj(JsObj::from_pairs(
                    NVIDIA_HEADERS
                        .iter()
                        .map(|(key, value)| (*key, Jv::s(*value)))
                        .collect(),
                )),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "compat" => Jv::Obj(nvidia_openai_compat()),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            });
            record_reasoning_options(&mut state.recorded, "nvidia", &live_model_id, model);
        }
    }

    // Process Together AI models
    let together_provider = ["together", "togetherai", "together-ai"]
        .iter()
        .find_map(|key| {
            document
                .get(key)
                .filter(|value| !value.is_null())
                .and_then(|provider| provider.get("models"))
                .and_then(Json::as_object)
        });
    if let Some(models) = together_provider {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            if doc_str(model, "status") == Some("deprecated") {
                continue;
            }
            let reasoning = doc_bool(model, "reasoning") == Some(true);
            let mut entry = JsObj::new();
            entry
                .set("id", Jv::s(model_id))
                .set("name", Jv::s(doc_str(model, "name").unwrap_or(model_id)))
                .set("api", Jv::s("openai-completions"))
                .set("provider", Jv::s("together"))
                .set("baseUrl", Jv::s(TOGETHER_BASE_URL))
                .set("reasoning", Jv::b(reasoning));
            if let Some(map) = get_together_thinking_level_map(model_id, reasoning) {
                entry.set("thinkingLevelMap", Jv::Obj(map));
            }
            entry
                .set("input", input_modalities(model))
                .set("cost", Jv::Obj(source_cost(model)))
                .set("compat", Jv::Obj(get_together_compat(model_id, reasoning)))
                .set("contextWindow", Jv::n(limit_or(model, "context", 4096.0)))
                .set("maxTokens", Jv::n(limit_or(model, "output", 4096.0)));
            state.models.push(entry);
            record_reasoning_options(&mut state.recorded, "together", model_id, model);
        }
    }

    process_baseten_models(section_models(document, "baseten"), state);

    // Process OpenCode models (Zen and Go). API mapping based on provider.npm:
    // - @ai-sdk/openai -> openai-responses
    // - @ai-sdk/anthropic -> anthropic-messages
    // - @ai-sdk/google -> google-generative-ai
    // - null/undefined/@ai-sdk/openai-compatible -> openai-completions
    let opencode_variants: [(&str, &str, &str); 2] = [
        ("opencode", "opencode", "https://opencode.ai/zen"),
        ("opencode-go", "opencode-go", "https://opencode.ai/zen/go"),
    ];
    for (key, provider, base_path) in opencode_variants {
        let Some(models) = section_models(document, key) else {
            continue;
        };
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            if doc_str(model, "status") == Some("deprecated") {
                continue;
            }

            let npm = model
                .get("provider")
                .and_then(|provider| doc_str(provider, "npm"));
            let mut api: &str = "openai-completions";
            let mut base_url: String = format!("{base_path}/v1");
            let mut compat: Option<JsObj> = None;

            match npm {
                Some("@ai-sdk/openai") => {
                    api = "openai-responses";
                    compat = Some(obj! { "sessionAffinityFormat" => Jv::s("openai-nosession") });
                }
                Some("@ai-sdk/anthropic") => {
                    api = "anthropic-messages";
                    // Anthropic SDK appends /v1/messages to baseURL
                    base_url = base_path.to_string();
                }
                Some("@ai-sdk/google") => {
                    api = "google-generative-ai";
                }
                Some("@ai-sdk/alibaba") => {
                    compat = Some(obj! { "cacheControlFormat" => Jv::s("anthropic") });
                }
                // null, undefined, or @ai-sdk/openai-compatible
                _ => {}
            }

            if provider == "opencode" && model_id == "grok-build-0.1" {
                let mut next = compat.take().unwrap_or_default();
                next.set("supportsReasoningEffort", Jv::b(false));
                compat = Some(next);
            }

            if (provider == "opencode" || provider == "opencode-go") && model_id == "kimi-k2.6" {
                // OpenCode Kimi K2.6 accepts Anthropic-style thinking objects
                // and rejects string thinking values or combined reasoning_effort.
                let mut next = compat.take().unwrap_or_default();
                next.set("thinkingFormat", Jv::s("deepseek"));
                next.set("supportsReasoningEffort", Jv::b(false));
                compat = Some(next);
            }

            // Fix known mismatches between models.dev npm data and actual
            // OpenCode Go endpoint behaviour. models.dev reports these models
            // as @ai-sdk/anthropic, but the OpenCode Go endpoints either don't
            // accept Anthropic SDK auth (MiniMax M2.7) or are served through
            // the OpenAI-compatible /v1/chat/completions path (Qwen 3.5/3.6).
            if provider == "opencode-go" {
                if model_id == "minimax-m2.7" {
                    api = "openai-completions";
                    base_url = format!("{base_path}/v1");
                }
                if model_id == "qwen3.5-plus" || model_id == "qwen3.6-plus" {
                    api = "openai-completions";
                    base_url = format!("{base_path}/v1");
                    // Qwen/DashScope uses enable_thinking at the top level.
                    let mut next = compat.take().unwrap_or_default();
                    next.set("thinkingFormat", Jv::s("qwen"));
                    compat = Some(next);
                }
            }

            if api == "openai-completions" {
                let mut next = compat.take().unwrap_or_default();
                next.set("maxTokensField", Jv::s("max_tokens"));
                if set_contains(
                    OPENCODE_LONG_CACHE_RETENTION_UNSUPPORTED_MODELS,
                    &format!("{provider}:{model_id}"),
                ) {
                    next.set("supportsLongCacheRetention", Jv::b(false));
                }
                compat = Some(next);
            }

            let thinking_level_map: Option<JsObj> = if api == "google-generative-ai" {
                google_thinking_level_map(model_id, model.get("reasoning_options"))
            } else if provider == "opencode-go" && model_id == "deepseek-v4.1-flash" {
                crate::reasoning_options::effort_thinking_level_map(
                    &crate::reasoning_options::parse_reasoning_options(
                        model.get("reasoning_options"),
                    ),
                )
            } else {
                None
            };

            let mut entry = JsObj::new();
            entry
                .set("id", Jv::s(model_id))
                .set("name", Jv::s(doc_str(model, "name").unwrap_or(model_id)))
                .set("api", Jv::s(api))
                .set("provider", Jv::s(provider))
                .set("baseUrl", Jv::s(&base_url))
                .set(
                    "reasoning",
                    Jv::b(doc_bool(model, "reasoning") == Some(true)),
                );
            if let Some(map) = thinking_level_map {
                entry.set("thinkingLevelMap", Jv::Obj(map));
            }
            entry
                .set("input", input_modalities(model))
                .set("cost", Jv::Obj(source_cost(model)));
            if let Some(compat) = compat {
                if !compat.is_empty() {
                    entry.set("compat", Jv::Obj(compat));
                }
            }
            entry
                .set("contextWindow", Jv::n(limit_or(model, "context", 4096.0)))
                .set("maxTokens", Jv::n(limit_or(model, "output", 4096.0)));
            state.models.push(entry);
            record_reasoning_options(&mut state.recorded, provider, model_id, model);
        }
    }

    // Process GitHub Copilot models
    if let Some(models) = section_models(document, "github-copilot") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            if doc_str(model, "status") == Some("deprecated") {
                continue;
            }

            // Claude 4.x and 5.x models route to Anthropic Messages API
            let is_copilot_claude = is_copilot_claude_id(model_id);
            // GPT, Grok, OSWE, and MAI-Code models are only served through
            // the Copilot /responses endpoint.
            let needs_responses_api = model_id.starts_with("gpt-")
                || model_id.starts_with("grok-")
                || model_id.starts_with("oswe")
                || model_id.starts_with("mai-");

            let api = if is_copilot_claude {
                "anthropic-messages"
            } else if needs_responses_api {
                "openai-responses"
            } else {
                "openai-completions"
            };

            let anthropic_compat = (api == "anthropic-messages")
                .then(|| get_anthropic_messages_compat("github-copilot", model_id))
                .flatten();

            let mut entry = object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s(api),
                "provider" => Jv::s("github-copilot"),
                "baseUrl" => Jv::s("https://api.individual.githubcopilot.com"),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(get_models_dev_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 128_000.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 8192.0)),
                "headers" => Jv::Obj(JsObj::from_pairs(
                    COPILOT_STATIC_HEADERS
                        .iter()
                        .map(|(key, value)| (*key, Jv::s(*value)))
                        .collect(),
                )),
            };
            if let Some(compat) = anthropic_compat {
                entry.set("compat", Jv::Obj(compat));
            }
            // compat only applies to openai-completions
            if api == "openai-completions" {
                entry.set(
                    "compat",
                    Jv::Obj(obj! {
                        "supportsStore" => Jv::b(false),
                        "supportsDeveloperRole" => Jv::b(false),
                        "supportsReasoningEffort" => Jv::b(false),
                    }),
                );
            }
            state.models.push(entry);
            record_reasoning_options(&mut state.recorded, "github-copilot", model_id, model);
        }
    }

    // Process MiniMax models
    let minimax_variants: [(&str, &str, &str); 2] = [
        ("minimax", "minimax", "https://api.minimax.io/anthropic"),
        (
            "minimax-cn",
            "minimax-cn",
            "https://api.minimaxi.com/anthropic",
        ),
    ];
    for (key, provider, base_url) in minimax_variants {
        if let Some(models) = section_models(document, key) {
            for (model_id, model) in models {
                if doc_bool(model, "tool_call") != Some(true) {
                    continue;
                }
                state.models.push(object! {
                    "id" => Jv::s(model_id),
                    "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                    "api" => Jv::s("anthropic-messages"),
                    "provider" => Jv::s(provider),
                    // MiniMax's Anthropic-compatible API - SDK appends /v1/messages
                    "baseUrl" => Jv::s(base_url),
                    "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                    "input" => input_modalities(model),
                    "cost" => Jv::Obj(source_cost(model)),
                    "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                    "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
                });
                record_reasoning_options(&mut state.recorded, provider, model_id, model);
            }
        }
    }

    process_kimi_coding_models(document, state);

    // Process Moonshot AI models
    let moonshot_variants: [(&str, &str, &str); 2] = [
        ("moonshotai", "moonshotai", "https://api.moonshot.ai/v1"),
        (
            "moonshotai-cn",
            "moonshotai-cn",
            "https://api.moonshot.cn/v1",
        ),
    ];
    for (key, provider, base_url) in moonshot_variants {
        if let Some(models) = section_models(document, key) {
            for (model_id, model) in models {
                if doc_bool(model, "tool_call") != Some(true) {
                    continue;
                }
                let is_kimi_k3 = model_id == "kimi-k3";
                let mut compat = moonshot_compat();
                if is_kimi_k3 {
                    compat.set("requiresReasoningContentOnAssistantMessages", Jv::b(true));
                    compat.set("thinkingFormat", Jv::s("openai"));
                    compat.set("supportsReasoningEffort", Jv::b(true));
                }
                let cost = model.get("cost");
                let kimi_fallback =
                    |field: &str| if is_kimi_k3 { kimi_k3_cost(field) } else { 0.0 };
                state.models.push(object! {
                    "id" => Jv::s(model_id),
                    "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                    "api" => Jv::s("openai-completions"),
                    "provider" => Jv::s(provider),
                    "baseUrl" => Jv::s(base_url),
                    "reasoning" => Jv::b(is_kimi_k3 || doc_bool(model, "reasoning") == Some(true)),
                    "input" => input_modalities(model),
                    "cost" => Jv::Obj(obj! {
                        "input" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "input")), kimi_fallback("input"))),
                        "output" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "output")), kimi_fallback("output"))),
                        "cacheRead" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "cache_read")), kimi_fallback("cacheRead"))),
                        "cacheWrite" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "cache_write")), kimi_fallback("cacheWrite"))),
                    }),
                    "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                    "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
                    "compat" => Jv::Obj(compat),
                });
                record_reasoning_options(&mut state.recorded, provider, model_id, model);
            }
        }
    }

    process_xiaomi_models(document, state);
    process_qwen_token_plan_models(document, state);
}

/// `KIMI_K3_COST[field]`.
fn kimi_k3_cost(field: &str) -> f64 {
    match field {
        "input" => 3.0,
        "output" => 15.0,
        "cacheRead" => 0.3,
        "cacheWrite" => 0.0,
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Per-provider process functions (generate-models.ts:1334-1656, 2306-2539)
// ---------------------------------------------------------------------------

/// `processGoogleModels`: the google provider's Gemini catalog plus the
/// Vertex Gemini subset (the models.dev google-vertex catalog also includes
/// Claude, OpenAI, and other MaaS models that do not use the @google/genai
/// Gemini streaming path).
fn process_google_models(document: &Json, state: &mut LoaderState) {
    if let Some(models) = section_models(document, "google") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            let source = gemini_latest_source(model_id, models, model);
            let entry = google_entry(
                model_id,
                model,
                "google-generative-ai",
                "google",
                "https://generativelanguage.googleapis.com/v1beta",
                source,
            );
            state.models.push(entry);
            record_reasoning_options(&mut state.recorded, "google", model_id, model);
        }
    }

    if let Some(models) = section_models(document, "google-vertex") {
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) || !model_id.starts_with("gemini-") {
                continue;
            }
            if model_id == "gemini-3.1-flash-lite-preview" {
                continue;
            }
            let source = gemini_latest_source(model_id, models, model);
            let mut entry = google_entry(
                model_id,
                model,
                "google-vertex",
                "google-vertex",
                VERTEX_BASE_URL,
                source,
            );
            // models.dev reports Vertex cache_read/cache_write values for
            // Gemini 2.5 Flash that do not match the official Gemini API
            // standard pricing table. pi only accounts
            // cachedContentTokenCount as cacheRead.
            let cache_read = if model_id == "gemini-2.5-flash" {
                0.03
            } else {
                js_or(
                    source
                        .get("cost")
                        .and_then(|cost| doc_f64(cost, "cache_read")),
                    0.0,
                )
            };
            if let Some(Jv::Obj(cost)) = entry.get_mut("cost") {
                cost.set("cacheRead", Jv::n(cache_read));
                cost.set("cacheWrite", Jv::n(0.0));
            }
            state.models.push(entry);
        }
    }
}

/// Upstream's `gemini-flash-latest`/`gemini-flash-lite-latest` source fallback
/// to the concrete generation entries.
fn gemini_latest_source<'a>(
    model_id: &str,
    models: &'a serde_json::Map<String, Json>,
    model: &'a Json,
) -> &'a Json {
    match model_id {
        "gemini-flash-latest" => models.get("gemini-3.5-flash").unwrap_or(model),
        "gemini-flash-lite-latest" => models.get("gemini-3.1-flash-lite").unwrap_or(model),
        _ => model,
    }
}

/// The shared Google entry literal (reasoning/modalities/cost/limits read
/// from `source`; the Vertex caller fixes cacheRead/cacheWrite afterwards).
fn google_entry(
    model_id: &str,
    model: &Json,
    api: &str,
    provider: &str,
    base_url: &str,
    source: &Json,
) -> Model {
    let mut entry = JsObj::new();
    entry
        .set("id", Jv::s(model_id))
        .set("name", Jv::s(doc_str(model, "name").unwrap_or(model_id)))
        .set("api", Jv::s(api))
        .set("provider", Jv::s(provider))
        .set("baseUrl", Jv::s(base_url))
        .set(
            "reasoning",
            Jv::b(doc_bool(source, "reasoning") == Some(true)),
        );
    if let Some(map) = google_thinking_level_map(model_id, source.get("reasoning_options")) {
        entry.set("thinkingLevelMap", Jv::Obj(map));
    }
    entry
        .set("input", input_modalities(source))
        .set("cost", Jv::Obj(source_cost(source)))
        .set("contextWindow", Jv::n(limit_or(source, "context", 4096.0)))
        .set("maxTokens", Jv::n(limit_or(source, "output", 4096.0)));
    entry
}

/// `processZaiModels` (generate-models.ts:1334-1392).
fn process_zai_models(document: &Json, state: &mut LoaderState) {
    let variants: [(&str, &str, &str); 2] = [
        (
            "zai-coding-plan",
            "zai",
            "https://api.z.ai/api/coding/paas/v4",
        ),
        (
            "zhipuai-coding-plan",
            "zai-coding-cn",
            "https://open.bigmodel.cn/api/coding/paas/v4",
        ),
    ];
    for (source_key, provider, base_url) in variants {
        let Some(models) = section_models(document, source_key) else {
            continue;
        };
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            let supports_image = model
                .get("modalities")
                .and_then(|modalities| modalities.get("input"))
                .and_then(Json::as_array)
                .is_some_and(|input| input.iter().any(|entry| entry.as_str() == Some("image")));

            let mut thinking_level_map = crate::reasoning_options::effort_thinking_level_map(
                &crate::reasoning_options::parse_reasoning_options(model.get("reasoning_options")),
            );
            let is_glm52 = model_id == "glm-5.2" || model_id == "glm-5.2-highspeed";
            if let (Some(map), true) = (thinking_level_map.as_mut(), is_glm52) {
                map.set("off", Jv::s("none"));
            }
            let supports_reasoning_effort = thinking_level_map.is_some();
            // The public zai section supplies the reference cost for both
            // coding-plan variants.
            let reference_cost = document
                .get("zai")
                .and_then(|zai| zai.get("models"))
                .and_then(|models| models.get(model_id))
                .and_then(|model| model.get("cost"))
                .or_else(|| model.get("cost"));

            let cost =
                |field: &str| js_or(reference_cost.and_then(|cost| doc_f64(cost, field)), 0.0);
            let mut compat = obj! {
                "supportsDeveloperRole" => Jv::b(false),
                "thinkingFormat" => Jv::s("zai"),
            };
            if supports_reasoning_effort {
                compat.set("supportsReasoningEffort", Jv::b(true));
            }
            if !set_contains(ZAI_TOOL_STREAM_UNSUPPORTED_MODELS, model_id) {
                compat.set("zaiToolStream", Jv::b(true));
            }

            let mut entry = JsObj::new();
            entry
                .set("id", Jv::s(model_id))
                .set("name", Jv::s(doc_str(model, "name").unwrap_or(model_id)))
                .set("api", Jv::s("openai-completions"))
                .set("provider", Jv::s(provider))
                .set("baseUrl", Jv::s(base_url))
                .set(
                    "reasoning",
                    Jv::b(doc_bool(model, "reasoning") == Some(true)),
                );
            if let Some(map) = thinking_level_map {
                entry.set("thinkingLevelMap", Jv::Obj(map));
            }
            entry
                .set(
                    "input",
                    if supports_image {
                        Jv::str_list(&["text", "image"])
                    } else {
                        Jv::str_list(&["text"])
                    },
                )
                .set(
                    "cost",
                    Jv::Obj(obj! {
                        "input" => Jv::n(cost("input")),
                        "output" => Jv::n(cost("output")),
                        "cacheRead" => Jv::n(cost("cache_read")),
                        "cacheWrite" => Jv::n(cost("cache_write")),
                    }),
                )
                .set("compat", Jv::Obj(compat))
                .set("contextWindow", Jv::n(limit_or(model, "context", 4096.0)))
                .set("maxTokens", Jv::n(limit_or(model, "output", 4096.0)));
            state.models.push(entry);
            record_reasoning_options(&mut state.recorded, provider, model_id, model);
        }
    }
}

/// `processFireworksModels` (generate-models.ts:1572-1656).
fn process_fireworks_models(
    models: Option<&serde_json::Map<String, Json>>,
    state: &mut LoaderState,
) {
    let Some(models) = models else {
        return;
    };
    for (model_id, model) in models {
        if doc_bool(model, "tool_call") != Some(true) {
            continue;
        }

        let is_glm = model_id.contains("glm-");
        let is_kimi_k3 = model_id.contains("kimi-k3");
        let compat = if is_glm {
            fireworks_openai_compat()
        } else if is_kimi_k3 {
            fireworks_kimi_k3_compat()
        } else {
            fireworks_anthropic_compat(model_id, model)
        };

        let entry = object! {
            "id" => Jv::s(model_id),
            "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
            "provider" => Jv::s("fireworks"),
            "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
            "input" => input_modalities(model),
            "cost" => Jv::Obj(source_cost(model)),
            "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
            "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            "api" => Jv::s(if is_glm || is_kimi_k3 { "openai-completions" } else { "anthropic-messages" }),
            // Fireworks Anthropic-compatible API - SDK appends /v1/messages.
            "baseUrl" => Jv::s(if is_glm || is_kimi_k3 {
                "https://api.fireworks.ai/inference/v1"
            } else {
                "https://api.fireworks.ai/inference"
            }),
            "compat" => Jv::Obj(compat),
        };
        state.models.push(entry);
        record_reasoning_options(&mut state.recorded, "fireworks", model_id, model);
    }
}

/// The Fireworks anthropic-messages compat, including the conditional
/// `forceAdaptiveThinking` (verified effort options or the #9323 fallbacks).
fn fireworks_anthropic_compat(model_id: &str, model: &Json) -> JsObj {
    let mut compat = obj! {
        "allowEmptySignature" => Jv::b(true),
        "sendSessionAffinityHeaders" => Jv::b(true),
        "supportsEagerToolInputStreaming" => Jv::b(false),
        "supportsCacheControlOnTools" => Jv::b(false),
        "supportsLongCacheRetention" => Jv::b(false),
    };
    let has_effort_option = model
        .get("reasoning_options")
        .and_then(Json::as_array)
        .is_some_and(|options| {
            options
                .iter()
                .any(|option| option.get("type").and_then(Json::as_str) == Some("effort"))
        });
    if has_effort_option || set_contains(FIREWORKS_ADAPTIVE_THINKING_FALLBACK_MODELS, model_id) {
        compat.set("forceAdaptiveThinking", Jv::b(true));
    }
    compat
}

fn fireworks_openai_compat() -> JsObj {
    obj! {
        "supportsStore" => Jv::b(false),
        "supportsDeveloperRole" => Jv::b(false),
        "sendSessionAffinityHeaders" => Jv::b(true),
        "supportsLongCacheRetention" => Jv::b(false),
    }
}

fn fireworks_kimi_k3_compat() -> JsObj {
    let mut compat = fireworks_openai_compat();
    compat.set("requiresReasoningContentOnAssistantMessages", Jv::b(true));
    compat.set("thinkingFormat", Jv::s("openai"));
    compat
}

/// `processBasetenModels` (generate-models.ts:1394-1492).
fn process_baseten_models(models: Option<&serde_json::Map<String, Json>>, state: &mut LoaderState) {
    let Some(models) = models else {
        return;
    };
    for (model_id, model) in models {
        if doc_str(model, "status") == Some("deprecated") {
            continue;
        }

        let reasoning = doc_bool(model, "reasoning") == Some(true);
        let reasoning_options =
            crate::reasoning_options::parse_reasoning_options(model.get("reasoning_options"));
        let is_glm52 = model_id == "zai-org/GLM-5.2" || model_id == "zai-org/GLM-5.2-Fast";
        let supports_toggle = reasoning_options.contains(&ReasoningOption::Toggle) || is_glm52;
        let supports_effort = reasoning_options
            .iter()
            .any(|option| matches!(option, ReasoningOption::Effort(_)))
            || is_glm52;

        let mut compat = baseten_base_compat();
        if supports_toggle && supports_effort {
            compat.spread(&{
                let mut next = baseten_reasoning_effort_compat();
                next.spread(&baseten_toggle_tail());
                next
            });
        } else if supports_toggle {
            compat.spread(&baseten_toggle_tail());
        } else if supports_effort {
            compat.spread(&baseten_reasoning_effort_compat());
        }

        let thinking_level_map = if is_glm52 {
            Some(level_map(&[
                ("off", Some("none")),
                ("minimal", None),
                ("low", None),
                ("medium", None),
                ("high", Some("high")),
                ("xhigh", None),
                ("max", Some("max")),
            ]))
        } else if supports_toggle {
            Some(level_map(&[
                ("off", Some("off")),
                ("minimal", None),
                ("low", None),
                ("medium", None),
                ("high", Some("high")),
                ("xhigh", None),
                ("max", None),
            ]))
        } else {
            crate::reasoning_options::effort_thinking_level_map(&reasoning_options)
        };
        // Baseten's GLM-5.2 endpoints are text-only despite models.dev
        // reporting image input.
        let supports_image_input = !is_glm52 && modalities_include_image(model);

        let mut entry = JsObj::new();
        entry
            .set("id", Jv::s(model_id))
            .set("name", Jv::s(doc_str(model, "name").unwrap_or(model_id)))
            .set("api", Jv::s("openai-completions"))
            .set("provider", Jv::s("baseten"))
            .set("baseUrl", Jv::s("https://inference.baseten.co/v1"))
            .set("reasoning", Jv::b(reasoning));
        if let Some(map) = thinking_level_map {
            entry.set("thinkingLevelMap", Jv::Obj(map));
        }
        entry
            .set(
                "input",
                if supports_image_input {
                    Jv::str_list(&["text", "image"])
                } else {
                    Jv::str_list(&["text"])
                },
            )
            .set("cost", Jv::Obj(source_cost(model)))
            .set("compat", Jv::Obj(compat))
            .set("contextWindow", Jv::n(limit_or(model, "context", 4096.0)))
            .set("maxTokens", Jv::n(limit_or(model, "output", 4096.0)));
        state.models.push(entry);
    }
}

/// The baseten compat spread order: `{...baseCompat, ...tail}` per variant.
fn baseten_base_compat() -> JsObj {
    obj! {
        "supportsStore" => Jv::b(false),
        "supportsDeveloperRole" => Jv::b(false),
        "supportsReasoningEffort" => Jv::b(false),
        "supportsUsageInStreaming" => Jv::b(true),
        "maxTokensField" => Jv::s("max_tokens"),
        "supportsStrictMode" => Jv::b(true),
        // Baseten automatic prompt caching needs session affinity so related
        // requests land on the same replica.
        "sendSessionAffinityHeaders" => Jv::b(true),
        "supportsLongCacheRetention" => Jv::b(false),
    }
}

fn baseten_reasoning_effort_compat() -> JsObj {
    let mut compat = baseten_base_compat();
    compat.set("supportsReasoningEffort", Jv::b(true));
    compat.set("thinkingFormat", Jv::s("openai"));
    compat
}

fn baseten_toggle_tail() -> JsObj {
    obj! {
        "thinkingFormat" => Jv::s("baseten"),
        "chatTemplateArgs" => Jv::Obj(obj! {
            "enable_thinking" => Jv::Obj(obj! { "$var" => Jv::s("thinking.enabled") }),
        }),
    }
}

/// `processKimiCodingModels` (generate-models.ts:2306-2350): the kimi-coding
/// provider. models.dev currently has no `kimi-for-coding` section (live
/// drift), so this produces nothing on real data — kept faithful so a future
/// regeneration includes the provider.
fn process_kimi_coding_models(document: &Json, state: &mut LoaderState) {
    let Some(models) = section_models(document, "kimi-for-coding") else {
        return;
    };
    let has_canonical_model = models.contains_key("kimi-for-coding");

    for (model_id, model) in models {
        if doc_bool(model, "tool_call") != Some(true) {
            continue;
        }
        // models.dev may expose versioned aliases (e.g. k2p5/k2p6/k2p7).
        // Normalize aliases to the canonical model id and drop duplicates
        // when the canonical entry exists.
        if set_contains(KIMI_ALIASES, model_id) && has_canonical_model {
            continue;
        }
        let normalized_id = if set_contains(KIMI_ALIASES, model_id) {
            "kimi-for-coding"
        } else {
            model_id
        };
        let normalized_name = if set_contains(KIMI_ALIASES, model_id) {
            "Kimi For Coding"
        } else {
            doc_str(model, "name").unwrap_or(normalized_id)
        };
        let is_kimi_k3 = normalized_id == "k3";
        let allow_empty_signature = is_kimi_k3 || normalized_id == "kimi-for-coding";
        let implied_cost = KIMI_CODING_IMPLIED_COSTS
            .iter()
            .find(|(id, _)| *id == normalized_id)
            .map(|(_, cost)| *cost);

        let implied = |field: &str| {
            implied_cost
                .and_then(|cost| {
                    cost.iter()
                        .find(|(name, _)| *name == field)
                        .map(|(_, value)| *value)
                })
                .unwrap_or(0.0)
        };
        let cost = model.get("cost");

        let mut compat = JsObj::new();
        if allow_empty_signature {
            compat.set("allowEmptySignature", Jv::b(true));
        }
        compat.set("forceAdaptiveThinking", Jv::b(true));

        state.models.push(object! {
            "id" => Jv::s(normalized_id),
            "name" => Jv::s(normalized_name),
            "api" => Jv::s("anthropic-messages"),
            "provider" => Jv::s("kimi-coding"),
            // Kimi For Coding's Anthropic-compatible API - SDK appends /v1/messages
            "baseUrl" => Jv::s("https://api.kimi.com/coding"),
            "compat" => Jv::Obj(compat),
            "reasoning" => Jv::b(is_kimi_k3 || doc_bool(model, "reasoning") == Some(true)),
            "input" => input_modalities(model),
            "cost" => Jv::Obj(obj! {
                "input" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "input")), implied("input"))),
                "output" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "output")), implied("output"))),
                "cacheRead" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "cache_read")), implied("cacheRead"))),
                "cacheWrite" => Jv::n(js_or(cost.and_then(|cost| doc_f64(cost, "cache_write")), implied("cacheWrite"))),
            }),
            "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
            "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
        });
        record_reasoning_options(&mut state.recorded, "kimi-coding", normalized_id, model);
    }
}

/// The Xiaomi MiMo loader (generate-models.ts:2407-2463). Built-in `xiaomi`
/// targets the API billing endpoint; the `xiaomi-token-plan-*` providers cover
/// the prepaid Token Plan endpoints in cn / ams / sgp.
fn process_xiaomi_models(document: &Json, state: &mut LoaderState) {
    let variants: [(&str, &str, &str); 4] = [
        ("xiaomi", "xiaomi", "https://api.xiaomimimo.com/v1"),
        (
            "xiaomi-token-plan-cn",
            "xiaomi-token-plan-cn",
            "https://token-plan-cn.xiaomimimo.com/v1",
        ),
        (
            "xiaomi-token-plan-ams",
            "xiaomi-token-plan-ams",
            "https://token-plan-ams.xiaomimimo.com/v1",
        ),
        (
            "xiaomi-token-plan-sgp",
            "xiaomi-token-plan-sgp",
            "https://token-plan-sgp.xiaomimimo.com/v1",
        ),
    ];
    for (source_key, provider, base_url) in variants {
        let Some(models) = section_models(document, source_key) else {
            continue;
        };
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            if doc_str(model, "status") == Some("deprecated") {
                continue;
            }
            state.models.push(object! {
                "id" => Jv::s(model_id),
                "name" => Jv::s(doc_str(model, "name").unwrap_or(model_id)),
                "api" => Jv::s("openai-completions"),
                "provider" => Jv::s(provider),
                "baseUrl" => Jv::s(base_url),
                "compat" => Jv::Obj(xiaomi_compat()),
                "reasoning" => Jv::b(doc_bool(model, "reasoning") == Some(true)),
                "input" => input_modalities(model),
                "cost" => Jv::Obj(source_cost(model)),
                "contextWindow" => Jv::n(limit_or(model, "context", 4096.0)),
                "maxTokens" => Jv::n(limit_or(model, "output", 4096.0)),
            });
            record_reasoning_options(&mut state.recorded, provider, model_id, model);
        }
    }
}

/// The Alibaba Cloud Model Studio Token Plan loader (generate-models.ts:2465-2539).
/// International and China use separate endpoints and API keys; the Individual
/// provider reuses the international source and endpoint with a narrower
/// catalog. models.dev keys are `alibaba-token-plan[-cn]`; pi exposes them as
/// `qwen-token-plan[-cn]` plus the Individual catalog view.
fn process_qwen_token_plan_models(document: &Json, state: &mut LoaderState) {
    let variants: [(&str, &str, &str, bool); 3] = [
        (
            "alibaba-token-plan",
            "qwen-token-plan",
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
            false,
        ),
        (
            "alibaba-token-plan",
            "qwen-token-plan-individual",
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
            true,
        ),
        (
            "alibaba-token-plan-cn",
            "qwen-token-plan-cn",
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            false,
        ),
    ];
    for (source_key, provider, base_url, allowlisted) in variants {
        let Some(models) = section_models(document, source_key) else {
            continue;
        };
        for (model_id, model) in models {
            if doc_bool(model, "tool_call") != Some(true) {
                continue;
            }
            if set_contains(QWEN_TOKEN_PLAN_EXCLUDED_MODEL_IDS, model_id) {
                continue;
            }
            if allowlisted && !set_contains(QWEN_TOKEN_PLAN_INDIVIDUAL_MODEL_IDS, model_id) {
                continue;
            }
            let thinking_level_map = crate::reasoning_options::effort_thinking_level_map(
                &crate::reasoning_options::parse_reasoning_options(model.get("reasoning_options")),
            )
            .or_else(|| {
                set_contains(
                    QWEN_TOKEN_PLAN_REASONING_EFFORT_FALLBACK_MODEL_IDS,
                    model_id,
                )
                .then(|| level_map(QWEN_TOKEN_PLAN_FALLBACK_THINKING_LEVEL_MAP))
            });

            let mut compat = qwen_token_plan_compat();
            if thinking_level_map.is_none() {
                compat.set("supportsReasoningEffort", Jv::b(false));
            }
            let mut entry = JsObj::new();
            entry
                .set("id", Jv::s(model_id))
                .set("name", Jv::s(doc_str(model, "name").unwrap_or(model_id)))
                .set("api", Jv::s("openai-completions"))
                .set("provider", Jv::s(provider))
                .set("baseUrl", Jv::s(base_url))
                .set("compat", Jv::Obj(compat));
            if let Some(map) = thinking_level_map {
                entry.set("thinkingLevelMap", Jv::Obj(map));
            }
            entry
                .set(
                    "reasoning",
                    Jv::b(doc_bool(model, "reasoning") == Some(true)),
                )
                .set("input", input_modalities(model))
                .set("cost", Jv::Obj(source_cost(model)))
                .set("contextWindow", Jv::n(limit_or(model, "context", 4096.0)))
                .set("maxTokens", Jv::n(limit_or(model, "output", 4096.0)));
            state.models.push(entry);
        }
    }
}

// ---------------------------------------------------------------------------
// Secondary fetches (generate-models.ts:1208-1332)
// ---------------------------------------------------------------------------

/// `fetchOpenRouterModels`: only tool-capable models, `anthropic/*` routed
/// through the Messages API, pricing converted from $/token to $/million.
pub(crate) fn fetch_openrouter_models(document: &Json, state: &mut LoaderState) {
    let Some(items) = document.get("data").and_then(Json::as_array) else {
        return;
    };
    for model in items {
        let Some(id) = doc_str(model, "id") else {
            continue;
        };
        let supported_parameters: Vec<&str> = model
            .get("supported_parameters")
            .and_then(Json::as_array)
            .map(|values| values.iter().filter_map(Json::as_str).collect())
            .unwrap_or_default();
        // Only include models that support tools
        if !supported_parameters.contains(&"tools") {
            continue;
        }

        // Parse input modalities
        let image = model
            .get("architecture")
            .and_then(|architecture| architecture.get("modality"))
            .and_then(Json::as_str)
            .is_some_and(|modality| modality.contains("image"));
        let pricing = model.get("pricing");
        let token_price = |field: &str| {
            round_cost(
                js_parse_optional(
                    pricing
                        .and_then(|pricing| pricing.get(field))
                        .and_then(Json::as_str),
                ) * 1_000_000.0,
            )
        };

        let context_window = model
            .get("top_provider")
            .and_then(|top| doc_f64(top, "context_length"))
            .or_else(|| doc_f64(model, "context_length"))
            .unwrap_or(4096.0);

        let use_anthropic_messages = id.starts_with("anthropic/") && !id.ends_with(":batch");
        let mut entry = JsObj::new();
        entry
            .set("id", Jv::s(id))
            .set("name", Jv::s(doc_str(model, "name").unwrap_or_default()))
            .set(
                "api",
                Jv::s(if use_anthropic_messages {
                    "anthropic-messages"
                } else {
                    "openai-completions"
                }),
            )
            .set(
                "baseUrl",
                Jv::s(if use_anthropic_messages {
                    "https://openrouter.ai/api"
                } else {
                    "https://openrouter.ai/api/v1"
                }),
            )
            .set("provider", Jv::s("openrouter"))
            .set(
                "reasoning",
                Jv::b(supported_parameters.contains(&"reasoning")),
            );
        if let Some(map) =
            crate::reasoning_options::openrouter_thinking_level_map(model.get("reasoning"))
        {
            entry.set("thinkingLevelMap", Jv::Obj(map));
        }
        entry
            .set(
                "input",
                if image {
                    Jv::str_list(&["text", "image"])
                } else {
                    Jv::str_list(&["text"])
                },
            )
            .set(
                "cost",
                Jv::Obj(obj! {
                    "input" => Jv::n(token_price("prompt")),
                    "output" => Jv::n(token_price("completion")),
                    "cacheRead" => Jv::n(token_price("input_cache_read")),
                    "cacheWrite" => Jv::n(token_price("input_cache_write")),
                }),
            )
            .set("contextWindow", Jv::n(context_window))
            .set(
                "maxTokens",
                Jv::n(
                    model
                        .get("top_provider")
                        .and_then(|top| doc_f64(top, "max_completion_tokens"))
                        .unwrap_or(4096.0),
                ),
            );
        state.models.push(entry);
    }
}

/// `fetchAiGatewayModels`: only `tool-use` tagged models through the
/// Anthropic-Messages unified API.
pub(crate) fn fetch_ai_gateway_models(document: &Json, state: &mut LoaderState) {
    let Some(items) = document.get("data").and_then(Json::as_array) else {
        return;
    };
    for model in items {
        let tags: Vec<&str> = model
            .get("tags")
            .and_then(Json::as_array)
            .map(|values| values.iter().filter_map(Json::as_str).collect())
            .unwrap_or_default();
        // Only include models that support tools
        if !tags.contains(&"tool-use") {
            continue;
        }
        let Some(id) = doc_str(model, "id") else {
            continue;
        };

        let pricing = model.get("pricing");
        let gateway_price = |field: &str| {
            round_cost(gateway_number(pricing.and_then(|pricing| pricing.get(field))) * 1_000_000.0)
        };

        state.models.push(object! {
            "id" => Jv::s(id),
            "name" => Jv::s(doc_str(model, "name").unwrap_or(id)),
            "api" => Jv::s("anthropic-messages"),
            "baseUrl" => Jv::s(crate::transform::AI_GATEWAY_BASE_URL),
            "provider" => Jv::s("vercel-ai-gateway"),
            "reasoning" => Jv::b(tags.contains(&"reasoning")),
            "input" => if tags.contains(&"vision") {
                Jv::str_list(&["text", "image"])
            } else {
                Jv::str_list(&["text"])
            },
            "compat" => Jv::Obj(obj! { "allowEmptySignature" => Jv::b(true) }),
            "cost" => Jv::Obj(obj! {
                "input" => Jv::n(gateway_price("input")),
                "output" => Jv::n(gateway_price("output")),
                "cacheRead" => Jv::n(gateway_price("input_cache_read")),
                "cacheWrite" => Jv::n(gateway_price("input_cache_write")),
            }),
            "contextWindow" => Jv::n(doc_f64(model, "context_window").filter(|value| *value != 0.0).unwrap_or(4096.0)),
            "maxTokens" => Jv::n(doc_f64(model, "max_tokens").filter(|value| *value != 0.0).unwrap_or(4096.0)),
        });
    }
}

/// The gateway `toNumber` (generate-models.ts:1281-1287): finite numbers pass
/// through, everything else parses as a float with a 0 fallback.
fn gateway_number(value: Option<&Json>) -> f64 {
    match value {
        Some(Json::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .unwrap_or(0.0),
        Some(Json::String(text)) => {
            let parsed = js_parse_f64(text);
            if parsed.is_finite() {
                parsed
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

/// `parseFloat(pricing?.prompt || "0")`.
fn js_parse_optional(text: Option<&str>) -> f64 {
    match text {
        Some(text) if !text.is_empty() => crate::json::js_parse_f64(text),
        _ => 0.0,
    }
}

/// `fetchNvidiaNimModelIds`: exact ids plus the lowercase/underscore-normalized
/// alias, both resolving to the live id.
pub(crate) fn nvidia_nim_model_ids(document: Option<&Json>) -> HashMap<String, String> {
    let mut model_ids = HashMap::new();
    let Some(items) = document
        .and_then(|document| document.get("data"))
        .and_then(Json::as_array)
    else {
        return model_ids;
    };
    for model in items {
        let Some(id) = doc_str(model, "id") else {
            continue;
        };
        model_ids.insert(id.to_string(), id.to_string());
        model_ids.insert(normalize_nvidia_model_id(id), id.to_string());
    }
    model_ids
}

/// The GitHub Copilot Claude regex `/^claude-(haiku|sonnet|opus|fable)-[45]([.\-]|$)/`.
fn is_copilot_claude_id(model_id: &str) -> bool {
    let Some(rest) = model_id.strip_prefix("claude-") else {
        return false;
    };
    let Some(rest) = ["haiku", "sonnet", "opus", "fable"]
        .iter()
        .find_map(|family| {
            rest.strip_prefix(family)
                .and_then(|rest| rest.strip_prefix('-'))
        })
    else {
        return false;
    };
    let Some(rest) = rest.strip_prefix('4').or_else(|| rest.strip_prefix('5')) else {
        return false;
    };
    rest.is_empty() || rest.starts_with('.') || rest.starts_with('-')
}

/// `getTogetherCompat` (generate-models.ts:518-524).
fn get_together_compat(model_id: &str, reasoning: bool) -> JsObj {
    if !reasoning {
        return together_base_compat();
    }
    if set_contains(TOGETHER_REASONING_EFFORT_MODELS, model_id) {
        return together_reasoning_effort_compat();
    }
    if set_contains(TOGETHER_TOGGLE_REASONING_EFFORT_MODELS, model_id) {
        let mut compat = together_toggle_reasoning_compat();
        compat.set("supportsReasoningEffort", Jv::b(true));
        return compat;
    }
    if set_contains(TOGETHER_REASONING_ONLY_MODELS, model_id) {
        return together_base_compat();
    }
    together_toggle_reasoning_compat()
}

/// `getTogetherThinkingLevelMap` (generate-models.ts:526-535).
fn get_together_thinking_level_map(model_id: &str, reasoning: bool) -> Option<JsObj> {
    if !reasoning {
        return None;
    }
    if set_contains(TOGETHER_REASONING_EFFORT_MODELS, model_id) {
        return Some(level_map(TOGETHER_REASONING_EFFORT_LEVEL_MAP));
    }
    if set_contains(TOGETHER_TOGGLE_REASONING_EFFORT_MODELS, model_id) {
        return Some(level_map(TOGETHER_DEEPSEEK_V4_THINKING_LEVEL_MAP));
    }
    if set_contains(TOGETHER_REASONING_ONLY_MODELS, model_id) {
        return Some(level_map(TOGETHER_FIXED_REASONING_LEVEL_MAP));
    }
    Some(level_map(TOGETHER_TOGGLE_REASONING_LEVEL_MAP))
}

fn together_base_compat() -> JsObj {
    obj! {
        "supportsStore" => Jv::b(false),
        "supportsDeveloperRole" => Jv::b(false),
        "supportsReasoningEffort" => Jv::b(false),
        "maxTokensField" => Jv::s("max_tokens"),
        "supportsStrictMode" => Jv::b(false),
        "supportsLongCacheRetention" => Jv::b(false),
    }
}

fn together_toggle_reasoning_compat() -> JsObj {
    let mut compat = together_base_compat();
    compat.set("thinkingFormat", Jv::s("together"));
    compat
}

fn together_reasoning_effort_compat() -> JsObj {
    let mut compat = together_base_compat();
    compat.set("supportsReasoningEffort", Jv::b(true));
    compat.set("thinkingFormat", Jv::s("openai"));
    compat
}

/// `getBedrockBaseUrl` (generate-models.ts:1148-1152).
fn bedrock_base_url(model_id: &str) -> &'static str {
    if model_id.starts_with("eu.") {
        "https://bedrock-runtime.eu-central-1.amazonaws.com"
    } else {
        "https://bedrock-runtime.us-east-1.amazonaws.com"
    }
}
