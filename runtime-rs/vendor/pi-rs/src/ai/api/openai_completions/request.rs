//! OpenAI-completions request assembly — full port of the message conversion
//! and request-body/headers assembly from upstream
//! `packages/ai/src/api/openai-completions.ts`:
//!
//! - `convertMessages` (lines 1185-1470) plus its dependency
//!   `transformMessages` (`api/transform-messages.ts`), the tool-call id
//!   normalizer (lines 1194-1218), and `shortHash`
//!   (`utils/hash.ts`).
//! - `buildParams` (lines 796-1002): sampling, thinking-format variants,
//!   thinking token budgets, prompt-cache fields, tools, tool choice, cache
//!   control markers, routing fields, and the trailing `samplingParams` merge.
//! - `convertTools` (lines 1472-1507) plus the constrained-sampling helpers of
//!   `api/constrained-sampling.ts` (`makeStrictJsonSchema`,
//!   `resolveJsonSchemaStrictSampling`, `resolveGrammarConstrainedSampling`).
//! - `createClient` header assembly (lines 751-794) without the HTTP client:
//!   the pi User-Agent, model headers, session-affinity headers, and the
//!   caller's headers merged last.
//! - `streamSimple` base-option shaping (`api/simple-options.ts`):
//!   `samplingParams` merge over `model.samplingParams`, context clamping of
//!   `maxTokens` (via the `utils/estimate.ts` port), and
//!   `clampThinkingLevel` (`models.ts:925-955`).
//!
//! The entry point is pure: it returns the JSON body and header pairs an HTTP
//! layer would send, it never performs I/O. Auth stays in the HTTP layer
//! (upstream `getClientApiKey`/SDK `Authorization` handling).
//!
//! Deviations from upstream, all structural:
//! - `sanitizeSurrogates` (`utils/sanitize-unicode.ts`) is a no-op: Rust
//!   `String` is UTF-8 and cannot hold unpaired surrogates, and serde_json
//!   rejects them at deserialization, so the JS cleanup pass has no input it
//!   could act on.
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order; servers do not care and every body assertion here is
//!   order-insensitive.
//! - Context-size estimation heuristics (`estimate.ts`) count Rust `char`s
//!   where JS counts UTF-16 code units; both are 4-chars-per-token estimates.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Map, Value};

use crate::ai::api::pi_user_agent;
use crate::ai::transcript::{
    content_text, get_declared_tools, get_system_message_text, resolve_transcript,
    resolve_transcript_tools, TranscriptContext,
};
use crate::ai::types::compat::{
    CacheControlFormat, MaxTokensField, OpenAiCompletionsCompat, ThinkingFormat,
    VercelGatewayRouting,
};
use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, SystemMessage, TextOrImageBlock,
    ToolResultMessage, UserMessage,
};
use crate::ai::types::options::{ProviderEnv, SimpleStreamOptions};
use crate::ai::types::primitives::{
    CacheRetention, ChatTemplateKwargValue, ChatTemplateVariable, SessionAffinityFormat,
    StopReason, ThinkingBudgets, ThinkingLevel, ThinkingTokenBudgetField, Usage,
};
use crate::ai::types::tool::{ConstrainedSampling, GrammarFormat, GrammarSampling, Strict, Tool};
use crate::ai::types::{Model, ModelInput};
use crate::ai::ProviderConfig;

/// The pure output of [`build_request`]: the JSON request body and the ordered
/// header pairs an HTTP layer would put on the wire (upstream assembles both
/// in `buildParams` + `createClient`).
#[derive(Debug, Clone, PartialEq)]
pub struct RequestAssembly {
    pub body: serde_json::Value,
    pub headers: Vec<(String, String)>,
}

/// Assemble the chat-completions request body and headers for one stream
/// request (upstream `buildParams` + `createClient` over the `streamSimple`
/// base options). Pure: no HTTP, no I/O.
///
/// `compat` is the resolved compatibility object — the caller applies
/// `detect_openai_completions_compat_for_model` + `merge_compat` (upstream
/// `getCompat`). `cfg.base_url` is the effective request endpoint; the
/// prompt-cache-key condition checks it for `api.openai.com` like upstream
/// checks `model.baseUrl`. `cfg.max_tokens`/`cfg.api_key` are not consumed
/// here: the default token cap is `model.maxTokens` (upstream
/// `buildBaseOptions`) and auth stays in the HTTP layer.
///
/// Errors mirror upstream throws: unsupported strict-sampling schemas,
/// grammar tools without a usable variant, and grammar tool calls whose
/// input property is not a string.
pub fn build_request(
    model: &Model,
    cfg: &ProviderConfig,
    ctx: &TranscriptContext,
    options: &SimpleStreamOptions,
    compat: &OpenAiCompletionsCompat,
) -> Result<RequestAssembly, String> {
    let grammar_tool_input_properties = create_grammar_tool_input_properties(
        &get_declared_tools(ctx.messages()),
        compat.supports_openai_grammar_tools == Some(true),
    )?;
    let cache_retention =
        resolve_cache_retention(options.stream.cache_retention, options.stream.env.as_ref());
    // Upstream: cacheSessionId = retention !== "none" ? options.sessionId : undefined.
    let cache_session_id = (cache_retention != CacheRetention::None)
        .then(|| options.stream.session_id.clone())
        .flatten();
    let headers = build_headers(model, options, compat, cache_session_id.as_deref());
    let body = build_params(
        model,
        cfg,
        ctx,
        options,
        compat,
        cache_retention,
        &grammar_tool_input_properties,
    )?;
    Ok(RequestAssembly { body, headers })
}

// ---- request body (upstream buildParams, openai-completions.ts:796-1002) ----

fn build_params(
    model: &Model,
    cfg: &ProviderConfig,
    ctx: &TranscriptContext,
    options: &SimpleStreamOptions,
    compat: &OpenAiCompletionsCompat,
    cache_retention: CacheRetention,
    grammar_tool_input_properties: &HashMap<String, String>,
) -> Result<Value, String> {
    let messages_slice = ctx.messages();
    let transcript_tools = resolve_transcript_tools(
        messages_slice,
        compat.supports_mid_convo_system_messages == Some(true)
            && compat.supports_mid_convo_tool_additions == Some(true),
    );
    let mut messages = convert_messages(model, ctx, compat, grammar_tool_input_properties)?;
    let cache_control = get_compat_cache_control(compat, cache_retention);

    // Tools before cache markers: upstream sets params.tools (line 848) and
    // applies the markers to it afterwards (line 858).
    let mut tools_value: Option<Value> = None;
    if !transcript_tools.request_tools.is_empty() {
        tools_value = Some(Value::Array(convert_tools(
            &transcript_tools.request_tools,
            compat,
        )?));
    } else if has_tool_history(messages_slice) {
        // Anthropic (via LiteLLM/proxy) requires the tools param when the
        // conversation has tool_calls/tool_results (upstream line 853).
        tools_value = Some(Value::Array(Vec::new()));
    }
    if let Some(marker) = &cache_control {
        apply_anthropic_cache_control(&mut messages, tools_value.as_mut(), marker);
    }

    let mut params = Map::new();
    params.insert("model".into(), Value::from(model.id.as_str()));
    params.insert("messages".into(), Value::Array(messages));
    params.insert("stream".into(), Value::from(true));

    // Prompt cache fields (upstream lines 820-825).
    let openai_direct = cfg.base_url.contains("api.openai.com");
    let long_retention = cache_retention == CacheRetention::Long
        && compat.supports_long_cache_retention == Some(true);
    if (openai_direct && cache_retention != CacheRetention::None) || long_retention {
        if let Some(key) = clamp_openai_prompt_cache_key(options.stream.session_id.as_deref()) {
            params.insert("prompt_cache_key".into(), Value::from(key.as_str()));
        }
    }
    if long_retention {
        params.insert("prompt_cache_retention".into(), Value::from("24h"));
    }

    if compat.supports_usage_in_streaming != Some(false) {
        params.insert(
            "stream_options".into(),
            serde_json::json!({"include_usage": true}),
        );
    }
    if compat.supports_store == Some(true) {
        params.insert("store".into(), Value::from(false));
    }

    // Upstream buildBaseOptions always fills maxTokens (clamped to the
    // remaining context window), so the field is always present and >= 1.
    let max_tokens = clamp_max_tokens_to_context(
        model,
        ctx,
        options.stream.max_tokens.unwrap_or(model.max_tokens),
    );
    if compat.max_tokens_field == Some(MaxTokensField::MaxTokens) {
        params.insert("max_tokens".into(), serde_json::json!(max_tokens));
    } else {
        params.insert(
            "max_completion_tokens".into(),
            serde_json::json!(max_tokens),
        );
    }

    if let Some(temperature) = options.stream.temperature {
        params.insert("temperature".into(), serde_json::json!(temperature));
    }

    if let Some(tools) = tools_value {
        if !transcript_tools.request_tools.is_empty() && compat.zai_tool_stream == Some(true) {
            params.insert("tool_stream".into(), Value::from(true));
        }
        params.insert("tools".into(), tools);
    }

    if let Some(tool_choice) = options.tool_choice {
        params.insert("tool_choice".into(), serde_json::json!(tool_choice));
    }

    if let Some(priority) = &compat.vllm_priority {
        params.insert("priority".into(), serde_json::json!(priority));
    }

    // Thinking budget machinery: the ceiling is the max-tokens field just
    // written (upstream resolveClampedThinkingBudget reads params).
    let ceiling = params
        .get("max_tokens")
        .and_then(Value::as_u64)
        .or_else(|| params.get("max_completion_tokens").and_then(Value::as_u64))
        .unwrap_or(model.max_tokens);
    let effort = options
        .reasoning
        .and_then(|level| clamp_thinking_level(model, Some(level)));
    let thinking_budget: Option<u64> = match effort {
        Some(level) if model.reasoning => {
            let budget = thinking_budget_for_level(level, options.thinking_budgets.as_ref());
            let clamped = budget.min(ceiling.saturating_sub(MIN_ANSWER_TOKENS));
            (clamped > 0).then_some(clamped)
        }
        _ => None,
    };

    apply_thinking_format(model, compat, effort, thinking_budget, &mut params);

    if let (Some(field), Some(budget)) =
        (resolve_thinking_token_budget_field(compat), thinking_budget)
    {
        params.insert(
            thinking_token_budget_field_name(field).to_string(),
            json!(budget),
        );
    }

    // OpenRouter provider routing preferences (upstream reads the RAW
    // model.compat, openai-completions.ts:981-983).
    if let Some(routing) = model
        .compat
        .as_ref()
        .and_then(|compat| compat.get("openRouterRouting"))
        .filter(|value| !value.is_null())
    {
        params.insert("provider".into(), routing.clone());
    }

    // Vercel AI Gateway provider routing preferences (lines 986-994).
    if let Some(routing) = model
        .compat
        .as_ref()
        .and_then(|compat| compat.get("vercelGatewayRouting"))
        .filter(|value| !value.is_null())
    {
        if let Ok(gateway) = serde_json::from_value::<VercelGatewayRouting>(routing.clone()) {
            if gateway.only.is_some() || gateway.order.is_some() {
                let mut options_object = Map::new();
                if let Some(only) = gateway.only {
                    options_object.insert("only".into(), json!(only));
                }
                if let Some(order) = gateway.order {
                    options_object.insert("order".into(), json!(order));
                }
                params.insert("providerOptions".into(), json!({"gateway": options_object}));
            }
        }
    }

    // Last so model and request sampling parameters override the named
    // request fields (upstream lines 1003-1006):
    // `resolveSamplingParams(model, options?.reasoningEffort ?? "off",
    // options?.samplingParams)` — the port's `options.reasoning` is the
    // effort surface, and the resolver clamps it to the model's supported
    // levels before the per-level lookup.
    if let Some(sampling) = resolve_sampling_params(
        model,
        options.reasoning,
        options.stream.sampling_params.as_ref(),
    ) {
        for (key, value) in sampling {
            params.insert(key, value);
        }
    }

    Ok(Value::Object(params))
}

// ---- thinking formats (upstream openai-completions.ts:870-1010) ----

/// Whether a thinking level survives `getSupportedThinkingLevels`
/// (`models.ts:925-933`).
fn supported_thinking_level(model: &Model, level: Option<ThinkingLevel>) -> bool {
    let mapped = model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(level_key(level)));
    match mapped {
        Some(Some(_)) => true,
        Some(None) => false,
        None => !matches!(level, Some(ThinkingLevel::Xhigh) | Some(ThinkingLevel::Max)),
    }
}

/// Upstream `clampThinkingLevel` (`models.ts:935-955`), which takes the full
/// `ModelThinkingLevel`: `None` is `"off"` on both the requested and returned
/// side.
pub(crate) fn clamp_thinking_level(
    model: &Model,
    requested: Option<ThinkingLevel>,
) -> Option<ThinkingLevel> {
    const EXTENDED: [Option<ThinkingLevel>; 7] = [
        None,
        Some(ThinkingLevel::Minimal),
        Some(ThinkingLevel::Low),
        Some(ThinkingLevel::Medium),
        Some(ThinkingLevel::High),
        Some(ThinkingLevel::Xhigh),
        Some(ThinkingLevel::Max),
    ];
    let available: Vec<Option<ThinkingLevel>> = if !model.reasoning {
        vec![None]
    } else {
        EXTENDED
            .iter()
            .copied()
            .filter(|level| supported_thinking_level(model, *level))
            .collect()
    };
    if available.contains(&requested) {
        return requested;
    }
    let requested_index = match requested {
        Some(requested) => EXTENDED.iter().position(|level| *level == Some(requested)),
        None => Some(0),
    };
    let Some(requested_index) = requested_index else {
        return available.first().copied().flatten();
    };
    for candidate in EXTENDED[requested_index..].iter() {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    for candidate in EXTENDED[..requested_index].iter().rev() {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    available.first().copied().flatten()
}

/// Upstream `resolveSamplingParams` (`api/simple-options.ts:24-35`): the
/// sampling parameters for one request — the model defaults
/// (`Model.samplingParams`), the per-level overrides selected by the
/// EFFECTIVE (clamped) pi thinking level
/// (`Model.samplingParamsByThinkingLevel`, `None` is `"off"`), and the
/// per-request keys, merged in that order. `None` when none of the three
/// sources is present, matching the upstream `||`-chain short-circuit, so
/// callers only override the named request fields when there is something to
/// apply.
pub(crate) fn resolve_sampling_params(
    model: &Model,
    thinking_level: Option<ThinkingLevel>,
    request_params: Option<&BTreeMap<String, serde_json::Value>>,
) -> Option<BTreeMap<String, serde_json::Value>> {
    let effective = clamp_thinking_level(model, thinking_level);
    let level_params = model
        .sampling_params_by_thinking_level
        .as_ref()
        .and_then(|by_level| by_level.get(level_key(effective)));
    if model.sampling_params.is_none() && level_params.is_none() && request_params.is_none() {
        return None;
    }
    let mut merged = model.sampling_params.clone().unwrap_or_default();
    if let Some(level_params) = level_params {
        for (key, value) in level_params {
            merged.insert(key.clone(), value.clone());
        }
    }
    if let Some(request_params) = request_params {
        for (key, value) in request_params {
            merged.insert(key.clone(), value.clone());
        }
    }
    Some(merged)
}

pub(crate) fn level_key(level: Option<ThinkingLevel>) -> &'static str {
    match level {
        None => "off",
        Some(ThinkingLevel::Minimal) => "minimal",
        Some(ThinkingLevel::Low) => "low",
        Some(ThinkingLevel::Medium) => "medium",
        Some(ThinkingLevel::High) => "high",
        Some(ThinkingLevel::Xhigh) => "xhigh",
        Some(ThinkingLevel::Max) => "max",
    }
}

fn level_str(level: ThinkingLevel) -> &'static str {
    level_key(Some(level))
}

/// `model.thinkingLevelMap[key]` with JS lookup semantics: absent vs `null` vs
/// a mapped string are all distinct upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MappedLevel {
    Absent,
    Null,
    Value(String),
}

pub(crate) fn map_level(model: &Model, key: &str) -> MappedLevel {
    match model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(key))
    {
        None => MappedLevel::Absent,
        Some(None) => MappedLevel::Null,
        Some(Some(value)) => MappedLevel::Value(value.clone()),
    }
}

/// Upstream `thinkingBudgetForLevel` + `clampReasoning`
/// (`api/simple-options.ts:57-72`): defaults overridable per level, xhigh/max
/// share the high budget.
pub(crate) fn thinking_budget_for_level(
    level: ThinkingLevel,
    custom: Option<&ThinkingBudgets>,
) -> u64 {
    let clamped = match level {
        ThinkingLevel::Xhigh | ThinkingLevel::Max => ThinkingLevel::High,
        other => other,
    };
    let custom = custom.cloned().unwrap_or_default();
    match clamped {
        ThinkingLevel::Minimal => custom.minimal.map(u64::from).unwrap_or(1024),
        ThinkingLevel::Low => custom.low.map(u64::from).unwrap_or(2048),
        ThinkingLevel::Medium => custom.medium.map(u64::from).unwrap_or(8192),
        _ => custom.high.map(u64::from).unwrap_or(16384),
    }
}

fn resolve_thinking_token_budget_field(
    compat: &OpenAiCompletionsCompat,
) -> Option<ThinkingTokenBudgetField> {
    if let Some(field) = compat.thinking_token_budget_field {
        return Some(field);
    }
    if compat.supports_thinking_token_budget == Some(true) {
        return Some(ThinkingTokenBudgetField::ThinkingTokenBudget);
    }
    None
}

fn thinking_token_budget_field_name(field: ThinkingTokenBudgetField) -> &'static str {
    match field {
        ThinkingTokenBudgetField::ThinkingTokenBudget => "thinking_token_budget",
        ThinkingTokenBudgetField::ThinkingBudget => "thinking_budget",
        ThinkingTokenBudgetField::ThinkingBudgetTokens => "thinking_budget_tokens",
    }
}

/// Resolved chat-template kwarg values keyed by name; `None` when nothing
/// survives (upstream `buildChatTemplateValues`).
fn build_chat_template_values(
    model: &Model,
    effort: Option<ThinkingLevel>,
    values: Option<&BTreeMap<String, ChatTemplateKwargValue>>,
    thinking_budget: Option<u64>,
) -> Option<Map<String, Value>> {
    let mut resolved = Map::new();
    for (key, value) in values.into_iter().flatten() {
        if let Some(value) =
            resolve_chat_template_kwarg_value(model, effort, value, thinking_budget)
        {
            resolved.insert(key.clone(), value);
        }
    }
    (!resolved.is_empty()).then_some(resolved)
}

/// Upstream `resolveChatTemplateKwargValue` (openai-completions.ts:1044-1067).
fn resolve_chat_template_kwarg_value(
    model: &Model,
    effort: Option<ThinkingLevel>,
    value: &ChatTemplateKwargValue,
    thinking_budget: Option<u64>,
) -> Option<Value> {
    match value {
        ChatTemplateKwargValue::Null => Some(Value::Null),
        ChatTemplateKwargValue::Bool(value) => Some(json!(value)),
        ChatTemplateKwargValue::Number(value) => Some(json!(value)),
        ChatTemplateKwargValue::String(value) => Some(json!(value)),
        ChatTemplateKwargValue::Variable { var, omit_when_off } => {
            if effort.is_none() && *omit_when_off == Some(true) {
                return None;
            }
            match var {
                ChatTemplateVariable::ThinkingEnabled => Some(json!(effort.is_some())),
                ChatTemplateVariable::ThinkingBudget => thinking_budget.map(|budget| json!(budget)),
                ChatTemplateVariable::ThinkingEffort => {
                    let mapped = match effort {
                        Some(level) => map_level(model, level_str(level)),
                        None => map_level(model, "off"),
                    };
                    match mapped {
                        MappedLevel::Value(mapped) => Some(json!(mapped)),
                        MappedLevel::Null => None,
                        MappedLevel::Absent => effort.map(|level| json!(level_str(level))),
                    }
                }
            }
        }
    }
}

/// Upstream thinking-format branch chain (openai-completions.ts:873-970).
fn apply_thinking_format(
    model: &Model,
    compat: &OpenAiCompletionsCompat,
    effort: Option<ThinkingLevel>,
    thinking_budget: Option<u64>,
    params: &mut Map<String, Value>,
) {
    let reasoning_on = model.reasoning;
    let supports_effort = compat.supports_reasoning_effort == Some(true);
    let effort_key = effort.map(level_str);
    // `model.thinkingLevelMap?.[key] ?? fallback`: null and absent fall back.
    let map_or = |key: &str, fallback: &str| -> String {
        match map_level(model, key) {
            MappedLevel::Value(value) => value,
            _ => fallback.to_string(),
        }
    };
    // Only a mapped string is used; null/absent are skipped.
    let mapped_only = |key: &str| -> Option<String> {
        match map_level(model, key) {
            MappedLevel::Value(value) => Some(value),
            _ => None,
        }
    };

    match compat.thinking_format {
        Some(ThinkingFormat::Zai) if reasoning_on => {
            let thinking = if effort.is_some() {
                json!({"type": "enabled", "clear_thinking": false})
            } else {
                json!({"type": "disabled"})
            };
            params.insert("thinking".into(), thinking);
            if let Some(key) = effort_key {
                if supports_effort {
                    // mapped === undefined ? effort : mapped; only strings land.
                    let value = match map_level(model, key) {
                        MappedLevel::Absent => Some(key.to_string()),
                        MappedLevel::Value(mapped) => Some(mapped),
                        MappedLevel::Null => None,
                    };
                    if let Some(value) = value {
                        params.insert("reasoning_effort".into(), json!(value));
                    }
                }
            }
        }
        Some(ThinkingFormat::Qwen) if reasoning_on => {
            params.insert("enable_thinking".into(), json!(effort.is_some()));
            if let Some(key) = effort_key {
                if supports_effort {
                    let value = map_or(key, key);
                    params.insert("reasoning_effort".into(), json!(value));
                }
            }
        }
        Some(ThinkingFormat::QwenChatTemplate) if reasoning_on => {
            params.insert(
                "chat_template_kwargs".into(),
                json!({"enable_thinking": effort.is_some(), "preserve_thinking": true}),
            );
        }
        Some(ThinkingFormat::ChatTemplate) if reasoning_on => {
            if let Some(kwargs) = build_chat_template_values(
                model,
                effort,
                compat.chat_template_kwargs.as_ref(),
                thinking_budget,
            ) {
                params.insert("chat_template_kwargs".into(), Value::Object(kwargs));
            }
        }
        Some(ThinkingFormat::Baseten) if reasoning_on => {
            if let Some(args) = build_chat_template_values(
                model,
                effort,
                compat.chat_template_args.as_ref(),
                thinking_budget,
            ) {
                params.insert("chat_template_args".into(), Value::Object(args));
            }
            if supports_effort {
                // mappedEffort = effort ? map[effort] : map["off"]; undefined
                // falls back to the requested effort.
                let requested = effort_key;
                let mapped = match requested {
                    Some(key) => map_level(model, key),
                    None => map_level(model, "off"),
                };
                let value = match mapped {
                    MappedLevel::Absent => requested.map(str::to_string),
                    MappedLevel::Value(mapped) => Some(mapped),
                    MappedLevel::Null => None,
                };
                if let Some(value) = value {
                    params.insert("reasoning_effort".into(), json!(value));
                }
            }
        }
        Some(ThinkingFormat::Deepseek) if reasoning_on => {
            if effort.is_some() {
                params.insert("thinking".into(), json!({"type": "enabled"}));
            } else if map_level(model, "off") != MappedLevel::Null {
                params.insert("thinking".into(), json!({"type": "disabled"}));
            }
            if let Some(key) = effort_key {
                if supports_effort {
                    let value = map_or(key, key);
                    params.insert("reasoning_effort".into(), json!(value));
                }
            }
        }
        Some(ThinkingFormat::Openrouter) if reasoning_on => {
            // OpenRouter normalizes reasoning across providers via a nested
            // reasoning object.
            if let Some(key) = effort_key {
                let value = map_or(key, key);
                params.insert("reasoning".into(), json!({"effort": value}));
            } else if map_level(model, "off") != MappedLevel::Null {
                let value = match map_level(model, "off") {
                    MappedLevel::Value(value) => value,
                    _ => "none".to_string(),
                };
                params.insert("reasoning".into(), json!({"effort": value}));
            }
        }
        Some(ThinkingFormat::AntLing) if reasoning_on && effort.is_some() => {
            if let Some(key) = effort_key {
                if let Some(value) = mapped_only(key) {
                    params.insert("reasoning".into(), json!({"effort": value}));
                }
            }
        }
        Some(ThinkingFormat::Together) if reasoning_on => {
            params.insert("reasoning".into(), json!({"enabled": effort.is_some()}));
            if let Some(key) = effort_key {
                if supports_effort {
                    let value = map_or(key, key);
                    params.insert("reasoning_effort".into(), json!(value));
                }
            }
        }
        Some(ThinkingFormat::StringThinking) if reasoning_on => {
            if let Some(key) = effort_key {
                let value = map_or(key, key);
                params.insert("thinking".into(), json!(value));
            } else if map_level(model, "off") != MappedLevel::Null {
                let value = match map_level(model, "off") {
                    MappedLevel::Value(value) => value,
                    _ => "none".to_string(),
                };
                params.insert("thinking".into(), json!(value));
            }
        }
        _ => {
            // OpenAI-style reasoning_effort (openai-completions.ts:962-970);
            // reached when the format is "openai" (or a guarded branch above
            // did not apply).
            if effort.is_some() && reasoning_on && supports_effort {
                if let Some(key) = effort_key {
                    let value = map_or(key, key);
                    params.insert("reasoning_effort".into(), json!(value));
                }
            } else if effort.is_none() && reasoning_on && supports_effort {
                if let Some(value) = mapped_only("off") {
                    params.insert("reasoning_effort".into(), json!(value));
                }
            }
        }
    }
}

// ---- tools (upstream convertTools + constrained-sampling.ts) ----

pub(crate) struct GrammarConstrainedSampling {
    pub(crate) format: &'static str,
    pub(crate) definition: String,
    pub(crate) input_property: String,
}

/// Upstream `resolveGrammarConstrainedSampling`
/// (`api/constrained-sampling.ts:230-263`). Shared with the openai-responses
/// port (`convertResponsesTools` custom-tool arm).
pub(crate) fn resolve_grammar_constrained_sampling(
    tool: &Tool,
    supports_openai_grammar_tools: bool,
) -> Result<Option<GrammarConstrainedSampling>, String> {
    let Some(ConstrainedSampling::Grammar(GrammarSampling { variants })) =
        &tool.constrained_sampling
    else {
        return Ok(None);
    };
    if !supports_openai_grammar_tools {
        return Ok(None);
    }
    let lark = variants.get(&GrammarFormat::Lark);
    let regex = variants.get(&GrammarFormat::Regex);
    let has_lark = lark.is_some_and(|definition| !definition.trim().is_empty());
    let has_regex = regex.is_some_and(|definition| !definition.trim().is_empty());
    if !has_lark && !has_regex {
        return Err(format!(
            "Tool \"{}\" cannot use grammar constrained sampling: no supported grammar variant was provided.",
            tool.name
        ));
    }
    let (format, definition) = if has_lark {
        ("lark", lark.cloned().unwrap_or_default())
    } else {
        ("regex", regex.cloned().unwrap_or_default())
    };
    let input_property = infer_grammar_input_property(tool).map_err(|message| {
        format!(
            "Tool \"{}\" cannot use grammar constrained sampling: {}.",
            tool.name, message
        )
    })?;
    Ok(Some(GrammarConstrainedSampling {
        format,
        definition,
        input_property,
    }))
}

/// Upstream `inferGrammarInputProperty`
/// (`api/constrained-sampling.ts:189-206`).
fn infer_grammar_input_property(tool: &Tool) -> Result<String, String> {
    if tool.parameters.get("type").and_then(Value::as_str) != Some("object") {
        return Err("grammar constrained sampling requires an object parameter schema".into());
    }
    let empty_properties = Map::new();
    let object = tool.parameters.as_object().unwrap_or(&empty_properties);
    let required = object.get("required").and_then(Value::as_array);
    let input_property = match required {
        Some(items) if items.len() == 1 && items[0].is_string() => {
            items[0].as_str().unwrap_or_default().to_string()
        }
        _ => {
            return Err(
                "grammar constrained sampling requires exactly one required string property".into(),
            )
        }
    };
    let property = object
        .get("properties")
        .and_then(|properties| properties.get(&input_property));
    let Some(property) = property else {
        return Err(format!(
            "grammar constrained sampling requires a properties entry for {input_property}"
        ));
    };
    if property.get("type").and_then(Value::as_str) != Some("string") {
        return Err(format!(
            "grammar constrained sampling property {input_property} must have type string"
        ));
    }
    Ok(input_property)
}

/// Upstream `createGrammarToolInputProperties`
/// (`api/constrained-sampling.ts:265-282`). Shared with the streaming port:
/// custom (grammar) tool-call deltas resolve their input property from the
/// same map the request builder uses.
pub(crate) fn create_grammar_tool_input_properties(
    tools: &[Tool],
    supports_openai_grammar_tools: bool,
) -> Result<HashMap<String, String>, String> {
    let mut properties = HashMap::new();
    for tool in tools {
        if let Some(grammar) =
            resolve_grammar_constrained_sampling(tool, supports_openai_grammar_tools)?
        {
            properties.insert(tool.name.clone(), grammar.input_property);
        }
    }
    Ok(properties)
}

/// Upstream `getGrammarToolInput` (`api/constrained-sampling.ts:145-155`).
/// Shared with the openai-responses port (custom_tool_call replay).
pub(crate) fn get_grammar_tool_input(
    tool_name: &str,
    arguments: &Value,
    input_property: &str,
) -> Result<String, String> {
    match arguments.get(input_property).and_then(Value::as_str) {
        Some(input) => Ok(input.to_string()),
        None => Err(format!(
            "Grammar tool call \"{tool_name}\" requires argument \"{input_property}\" to be a string."
        )),
    }
}

/// Upstream `UnsupportedStrictSchemaKeywordCheck` (constrained-sampling.ts):
/// returns true when a provider's strict mode rejects this schema keyword
/// with this value, so "prefer" tools fall back to non-strict.
pub(crate) type UnsupportedStrictSchemaKeywordCheck = fn(&str, &Value) -> bool;

const UNSUPPORTED_STRICT_SCHEMA_KEYS: [&str; 16] = [
    "$ref",
    "$defs",
    "definitions",
    "allOf",
    "oneOf",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "prefixItems",
    "not",
    "if",
    "then",
    "else",
];

/// Upstream `makeStrictJsonSchema` (`api/constrained-sampling.ts:117-130`):
/// convert a tool schema to the strict subset providers accept. The optional
/// keyword check lets a provider reject extra keywords its strict mode does
/// not accept (the Anthropic delta).
pub(crate) fn make_strict_json_schema(
    schema: &Value,
    is_unsupported_keyword: Option<UnsupportedStrictSchemaKeywordCheck>,
) -> Result<Value, String> {
    let mut cloned = schema.clone();
    if !cloned.is_object() {
        return Err("root schema must have type object".into());
    }
    make_schema_node_strict(&mut cloned, is_unsupported_keyword)?;
    if cloned.get("type").and_then(Value::as_str) != Some("object") {
        return Err("root schema must have type object".into());
    }
    Ok(cloned)
}

/// Upstream `isStructuredSchema` (`api/constrained-sampling.ts:35-44`).
fn is_structured_schema(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    let types: Vec<&str> = match object.get("type") {
        Some(Value::String(single)) => vec![single.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    types.contains(&"object")
        || types.contains(&"array")
        || object.contains_key("properties")
        || object.contains_key("items")
}

/// Upstream `schemaAllowsNull` (`api/constrained-sampling.ts:46-51`).
fn schema_allows_null(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) == Some("null") {
        return true;
    }
    if let Some(Value::Array(types)) = object.get("type") {
        if types.iter().any(|value| value == &json!("null")) {
            return true;
        }
    }
    if object.get("const").is_some_and(Value::is_null) {
        return true;
    }
    if let Some(Value::Array(values)) = object.get("enum") {
        if values.iter().any(Value::is_null) {
            return true;
        }
    }
    matches!(object.get("anyOf"), Some(Value::Array(variants)) if variants.iter().any(schema_allows_null))
}

/// Upstream `makeJsonSchemaNodeStrict` (`api/constrained-sampling.ts:53-114`).
fn make_schema_node_strict(
    schema: &mut Value,
    is_unsupported_keyword: Option<UnsupportedStrictSchemaKeywordCheck>,
) -> Result<(), String> {
    if !schema.is_object() {
        return Err("boolean schemas are unsupported".into());
    }
    for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
        if schema.as_object().unwrap_or(&Map::new()).contains_key(key) {
            return Err(format!("{key} schemas are unsupported"));
        }
    }
    // Upstream (the Anthropic delta): the provider-specific keyword probes
    // run over the raw schema entries, after the shared unsupported-keys
    // check and before anyOf/items/properties recursion.
    if let Some(is_unsupported_keyword) = is_unsupported_keyword {
        for (key, value) in schema.as_object().unwrap_or(&Map::new()) {
            if is_unsupported_keyword(key, value) {
                return Err(format!(
                    "{key}: {} is unsupported",
                    serde_json::to_string(value).unwrap_or_default()
                ));
            }
        }
    }
    let object = schema.as_object_mut().unwrap();
    if let Some(any_of) = object.get_mut("anyOf") {
        let Value::Array(variants) = any_of else {
            return Err("anyOf must contain at least one schema".into());
        };
        if variants.is_empty() {
            return Err("anyOf must contain at least one schema".into());
        }
        for variant in variants.iter() {
            if is_structured_schema(variant) {
                return Err("object and array unions are unsupported".into());
            }
        }
        for variant in variants.iter_mut() {
            make_schema_node_strict(variant, is_unsupported_keyword)?;
        }
    }
    if let Some(items) = object.get_mut("items") {
        if items.is_array() {
            return Err("tuple schemas are unsupported".into());
        }
        make_schema_node_strict(items, is_unsupported_keyword)?;
    }

    let is_object_schema = object.get("type").and_then(Value::as_str) == Some("object");
    if object.contains_key("properties") && !is_object_schema {
        return Err("properties require type object".into());
    }
    if !is_object_schema {
        return Ok(());
    }
    if let Some(additional) = object.get("additionalProperties") {
        if additional != &json!(false) {
            return Err("schema-valued or true additionalProperties is unsupported".into());
        }
    }
    if let Some(properties) = object.get("properties") {
        if !properties.is_object() {
            return Err("object properties must be a schema map".into());
        }
    }
    if let Some(required) = object.get("required") {
        let valid = required
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string));
        if !valid {
            return Err("object required must be a string array".into());
        }
    }

    let property_names: Vec<String> = object
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default();
    let required_list: Vec<String> = object
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    for key in &required_list {
        if !property_names.contains(key) {
            return Err("required contains an unknown property".into());
        }
    }

    if let Some(properties) = object.get_mut("properties") {
        let properties = properties.as_object_mut().unwrap();
        for property in properties.values_mut() {
            make_schema_node_strict(property, is_unsupported_keyword)?;
        }
        for (key, property) in properties.iter_mut() {
            if !required_list.contains(key) && !schema_allows_null(property) {
                *property = json!({"anyOf": [property.clone(), {"type": "null"}]});
            }
        }
    }
    object.insert(
        "required".into(),
        Value::Array(property_names.into_iter().map(Value::String).collect()),
    );
    object.insert("additionalProperties".into(), json!(false));
    Ok(())
}

/// Upstream `resolveJsonSchemaStrictSampling`
/// (`api/constrained-sampling.ts:208-228`): `Some(true)` when the tool
/// demands (and supports) strict sampling, `None` for a plain fallback,
/// `Err` when a "require" request cannot be honored.
pub(crate) fn resolve_json_schema_strict_sampling(
    tool: &Tool,
    supports_strict_mode: bool,
    is_unsupported_keyword: Option<UnsupportedStrictSchemaKeywordCheck>,
) -> Result<Option<bool>, String> {
    let Some(ConstrainedSampling::JsonSchema(config)) = &tool.constrained_sampling else {
        return Ok(None);
    };
    if supports_strict_mode {
        return match make_strict_json_schema(&tool.parameters, is_unsupported_keyword) {
            Ok(_) => Ok(Some(true)),
            Err(message) => {
                if config.strict != Strict::Require {
                    return Ok(None);
                }
                Err(format!(
                    "Tool \"{}\" requires JSON-schema constrained sampling, but {}.",
                    tool.name, message
                ))
            }
        };
    }
    if config.strict == Strict::Require {
        return Err(format!(
            "Tool \"{}\" requires JSON-schema constrained sampling, but strict tools are unsupported.",
            tool.name
        ));
    }
    Ok(None)
}

/// Upstream `convertTools` (openai-completions.ts:1472-1507).
fn convert_tools(tools: &[Tool], compat: &OpenAiCompletionsCompat) -> Result<Vec<Value>, String> {
    let supports_strict = compat.supports_strict_mode != Some(false);
    let supports_grammar = compat.supports_openai_grammar_tools == Some(true);
    tools
        .iter()
        .map(|tool| {
            if let Some(grammar) = resolve_grammar_constrained_sampling(tool, supports_grammar)? {
                return Ok(json!({
                    "type": "custom",
                    "custom": {
                        "name": tool.name,
                        "description": tool.description,
                        "format": {
                            "type": "grammar",
                            "grammar": {"syntax": grammar.format, "definition": grammar.definition}
                        }
                    }
                }));
            }

            let strict = resolve_json_schema_strict_sampling(tool, supports_strict, None)?;
            let mut function = Map::new();
            function.insert("name".into(), Value::from(tool.name.as_str()));
            function.insert("description".into(), Value::from(tool.description.as_str()));
            function.insert(
                "parameters".into(),
                if strict == Some(true) {
                    make_strict_json_schema(&tool.parameters, None)?
                } else {
                    tool.parameters.clone()
                },
            );
            // Only include strict if provider supports it. Some reject unknown
            // fields (upstream line 1502-1503).
            if supports_strict {
                function.insert("strict".into(), json!(strict.unwrap_or(false)));
            }
            Ok(json!({"type": "function", "function": function}))
        })
        .collect()
}

// ---- anthropic cache control (upstream openai-completions.ts:1069-1183) ----

fn cache_control_marker(ttl: Option<&str>) -> Value {
    match ttl {
        Some(ttl) => json!({"type": "ephemeral", "ttl": ttl}),
        None => json!({"type": "ephemeral"}),
    }
}

/// Upstream `getCompatCacheControl` (openai-completions.ts:1069-1079).
fn get_compat_cache_control(
    compat: &OpenAiCompletionsCompat,
    cache_retention: CacheRetention,
) -> Option<Value> {
    if compat.cache_control_format != Some(CacheControlFormat::Anthropic)
        || cache_retention == CacheRetention::None
    {
        return None;
    }
    let ttl = (cache_retention == CacheRetention::Long
        && compat.supports_long_cache_retention == Some(true))
    .then_some("1h");
    Some(cache_control_marker(ttl))
}

/// Upstream `applyAnthropicCacheControl` (openai-completions.ts:1081-1089).
fn apply_anthropic_cache_control(
    messages: &mut [Value],
    tools: Option<&mut Value>,
    marker: &Value,
) {
    add_cache_control_to_system_prompt(messages, marker);
    add_cache_control_to_last_tool(tools, marker);
    add_cache_control_to_last_conversation_message(messages, marker);
}

fn add_cache_control_to_system_prompt(messages: &mut [Value], marker: &Value) {
    for message in messages.iter_mut() {
        if matches!(
            message.get("role").and_then(Value::as_str),
            Some("system") | Some("developer")
        ) {
            add_cache_control_to_text_content(message, marker);
            return;
        }
    }
}

fn add_cache_control_to_last_conversation_message(messages: &mut [Value], marker: &Value) {
    for message in messages.iter_mut().rev() {
        if matches!(
            message.get("role").and_then(Value::as_str),
            Some("user") | Some("assistant") | Some("tool")
        ) && add_cache_control_to_text_content(message, marker)
        {
            return;
        }
    }
}

fn add_cache_control_to_last_tool(tools: Option<&mut Value>, marker: &Value) {
    let Some(Value::Array(tools)) = tools else {
        return;
    };
    let Some(last_tool) = tools.last_mut() else {
        return;
    };
    if let Some(object) = last_tool.as_object_mut() {
        object.insert("cache_control".into(), marker.clone());
    }
}

/// Upstream `addCacheControlToTextContent`
/// (openai-completions.ts:1146-1183): convert string content to a one-part
/// array, or mark the last text part of an array. Returns whether a marker
/// landed.
fn add_cache_control_to_text_content(message: &mut Value, marker: &Value) -> bool {
    let updated = match message.get("content") {
        Some(Value::String(text)) => {
            if text.is_empty() {
                None
            } else {
                Some(Value::Array(vec![json!({
                    "type": "text",
                    "text": text,
                    "cache_control": marker
                })]))
            }
        }
        Some(Value::Array(parts)) => {
            let mut parts = parts.clone();
            let mut marked = false;
            for part in parts.iter_mut().rev() {
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(part_object) = part.as_object_mut() {
                        part_object.insert("cache_control".into(), marker.clone());
                    }
                    marked = true;
                    break;
                }
            }
            marked.then_some(Value::Array(parts))
        }
        _ => None,
    };
    match updated {
        Some(content) => {
            if let Some(object) = message.as_object_mut() {
                object.insert("content".into(), content);
                true
            } else {
                false
            }
        }
        None => false,
    }
}

// ---- headers (upstream createClient, openai-completions.ts:751-794) ----

pub(crate) fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    match headers.iter_mut().find(|(key, _)| key == name) {
        Some(entry) => entry.1 = value.to_string(),
        None => headers.push((name.to_string(), value.to_string())),
    }
}

pub(crate) fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(key, _)| key != name);
}

fn build_headers(
    model: &Model,
    options: &SimpleStreamOptions,
    compat: &OpenAiCompletionsCompat,
    session_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = vec![("User-Agent".to_string(), pi_user_agent())];
    for (name, value) in model.headers.iter().flatten() {
        match value {
            Some(value) => set_header(&mut headers, name, value),
            None => remove_header(&mut headers, name),
        }
    }
    if let Some(session_id) = session_id {
        if compat.send_session_affinity_headers == Some(true) {
            if compat.session_affinity_format == Some(SessionAffinityFormat::Openrouter) {
                set_header(&mut headers, "x-session-id", session_id);
            } else {
                if compat.session_affinity_format == Some(SessionAffinityFormat::Openai) {
                    set_header(&mut headers, "session_id", session_id);
                }
                set_header(&mut headers, "x-client-request-id", session_id);
                set_header(&mut headers, "x-session-affinity", session_id);
            }
        }
    }
    // Merge options headers last so they can override defaults; a `None`
    // value (upstream `null`) suppresses a default header.
    if let Some(option_headers) = &options.stream.headers {
        for (name, value) in option_headers {
            match value {
                Some(value) => set_header(&mut headers, name, value),
                None => remove_header(&mut headers, name),
            }
        }
    }
    headers
}

// ---- cache retention + prompt cache key ----

/// Upstream `resolveCacheRetention` (openai-completions.ts:289-297) +
/// `getProviderEnvValue` (`utils/provider-env.ts`): explicit option, then the
/// scoped env map, then the process environment, then `"short"`.
pub(crate) fn resolve_cache_retention(
    retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> CacheRetention {
    if let Some(retention) = retention {
        return retention;
    }
    let from_env = env
        .and_then(|env| env.get("PI_CACHE_RETENTION"))
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| {
            std::env::var("PI_CACHE_RETENTION")
                .ok()
                .filter(|value| !value.is_empty())
        });
    if from_env.as_deref() == Some("long") {
        CacheRetention::Long
    } else {
        CacheRetention::Short
    }
}

/// Upstream `clampOpenAIPromptCacheKey`
/// (`api/openai-prompt-cache.ts`): clamp by code points to 64.
const OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH: usize = 64;

pub(crate) fn clamp_openai_prompt_cache_key(key: Option<&str>) -> Option<String> {
    key.map(|key| {
        key.chars()
            .take(OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH)
            .collect()
    })
}

/// Upstream `hasToolHistory` (openai-completions.ts:88-100).
fn has_tool_history(messages: &[Message]) -> bool {
    messages.iter().any(|message| match message {
        Message::ToolResult(_) => true,
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .any(|block| matches!(block, AssistantBlock::ToolCall(_))),
        _ => false,
    })
}

// ---- max-tokens clamping (upstream api/simple-options.ts + utils/estimate.ts) ----

const CHARS_PER_TOKEN: usize = 4;
const ESTIMATED_IMAGE_CHARS: usize = 4800;
const CONTEXT_SAFETY_TOKENS: u64 = 4096;
const MIN_MAX_TOKENS: u64 = 1;
/// Tokens always left for the answer when a thinking budget shares the
/// response ceiling (`api/simple-options.ts:55`).
pub(crate) const MIN_ANSWER_TOKENS: u64 = 1024;

fn estimate_text_tokens(text: &str) -> u64 {
    text.chars().count().div_ceil(CHARS_PER_TOKEN) as u64
}

/// Upstream `calculateContextTokens` (`utils/estimate.ts:22`).
fn calculate_context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input + usage.output + usage.cache_read + usage.cache_write
    }
}

fn estimate_blocks_tokens(blocks: &[TextOrImageBlock]) -> u64 {
    let mut chars = 0usize;
    for block in blocks {
        match block {
            TextOrImageBlock::Text(text) => chars += text.text.chars().count(),
            TextOrImageBlock::Image(_) => chars += ESTIMATED_IMAGE_CHARS,
        }
    }
    chars.div_ceil(CHARS_PER_TOKEN) as u64
}

fn estimate_user_content_tokens(content: &StringOrBlocks) -> u64 {
    match content {
        StringOrBlocks::Text(text) => estimate_text_tokens(text),
        StringOrBlocks::Blocks(blocks) => estimate_blocks_tokens(blocks),
    }
}

fn estimate_tools_tokens<T: serde::Serialize>(tools: Option<&Vec<T>>) -> u64 {
    match tools {
        Some(tools) if !tools.is_empty() => {
            let serialized = serde_json::to_string(tools).unwrap_or_default();
            estimate_text_tokens(&serialized)
        }
        _ => 0,
    }
}

/// Upstream `estimateMessageTokens` (`utils/estimate.ts:58-84`).
fn estimate_message_tokens(message: &Message) -> u64 {
    match message {
        Message::System(system) => {
            estimate_text_tokens(&get_system_message_text(system))
                + estimate_tools_tokens(system.tools_added.as_ref())
                + estimate_tools_tokens(system.tools_removed.as_ref())
        }
        Message::User(user) => estimate_user_content_tokens(&user.content),
        Message::ToolResult(result) => estimate_blocks_tokens(&result.content),
        Message::Assistant(assistant) => {
            let mut chars = 0usize;
            for block in &assistant.content {
                match block {
                    AssistantBlock::Text(text) => chars += text.text.chars().count(),
                    AssistantBlock::Thinking(thinking) => {
                        chars += thinking.thinking.chars().count()
                    }
                    AssistantBlock::ToolCall(call) => {
                        chars += call.name.chars().count()
                            + serde_json::to_string(&call.arguments)
                                .map(|serialized| serialized.chars().count())
                                .unwrap_or(0)
                    }
                }
            }
            chars.div_ceil(CHARS_PER_TOKEN) as u64
        }
    }
}

/// Upstream `getLastAssistantUsageInfo` (`utils/estimate.ts:86-112`).
fn last_assistant_usage(messages: &[Message]) -> Option<(&Usage, usize)> {
    let mut latest_prefix_timestamp = i64::MIN;
    let mut info: Option<(&Usage, usize)> = None;
    for (index, message) in messages.iter().enumerate() {
        if let Message::Assistant(assistant) = message {
            let usage_applies_to_prefix = assistant.timestamp >= latest_prefix_timestamp;
            if usage_applies_to_prefix
                && assistant.stop_reason != StopReason::Aborted
                && assistant.stop_reason != StopReason::Error
                && calculate_context_tokens(&assistant.usage) > 0
            {
                info = Some((&assistant.usage, index));
            }
        }
        let timestamp = match message {
            Message::System(system) => system.timestamp,
            Message::User(user) => user.timestamp,
            Message::Assistant(assistant) => assistant.timestamp,
            Message::ToolResult(result) => result.timestamp,
        };
        latest_prefix_timestamp = latest_prefix_timestamp.max(timestamp);
    }
    info
}

/// Upstream `estimateContextTokens` (`utils/estimate.ts:114-131`), token sum.
fn estimate_context_tokens(messages: &[Message]) -> u64 {
    match last_assistant_usage(messages) {
        Some((usage, index)) => {
            let usage_tokens = calculate_context_tokens(usage);
            let trailing: u64 = messages[index + 1..]
                .iter()
                .map(estimate_message_tokens)
                .sum();
            usage_tokens + trailing
        }
        None => messages.iter().map(estimate_message_tokens).sum(),
    }
}

/// Upstream `clampMaxTokensToContext` (`api/simple-options.ts:15-19`).
pub(crate) fn clamp_max_tokens_to_context(
    model: &Model,
    ctx: &TranscriptContext,
    max_tokens: u64,
) -> u64 {
    if model.context_window == 0 {
        return max_tokens.max(MIN_MAX_TOKENS);
    }
    let available = model
        .context_window
        .saturating_sub(estimate_context_tokens(ctx.messages()))
        .saturating_sub(CONTEXT_SAFETY_TOKENS);
    max_tokens.min(available.max(MIN_MAX_TOKENS))
}

// ---- hashing + tool-call id normalization ----

/// Upstream `shortHash` (`utils/hash.ts`): 32-bit arithmetic with `Math.imul`
/// and a base-36 tail of both halves. UTF-16 code units are hashed, matching
/// JS `charCodeAt`. Shared with the openai-responses port (foreign item-id
/// hashing and `pi_tool_load_` call ids).
pub(crate) fn short_hash(input: &str) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    fn base36(mut value: u32) -> String {
        if value == 0 {
            return "0".to_string();
        }
        let mut digits = Vec::new();
        while value > 0 {
            digits.push(DIGITS[(value % 36) as usize]);
            value /= 36;
        }
        digits.reverse();
        String::from_utf8(digits).unwrap_or_default()
    }
    let mut h1: u32 = 0xdeadbeef;
    let mut h2: u32 = 0x41c6ce57;
    for unit in input.encode_utf16() {
        let ch = u32::from(unit);
        h1 = (h1 ^ ch).wrapping_mul(2654435761);
        h2 = (h2 ^ ch).wrapping_mul(1597334677);
    }
    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2246822507) ^ (h2 ^ (h2 >> 13)).wrapping_mul(3266489909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2246822507) ^ (h1 ^ (h1 >> 13)).wrapping_mul(3266489909);
    format!("{}{}", base36(h2), base36(h1))
}

fn sanitize_tool_id_part(part: &str) -> String {
    part.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Upstream `normalizeToolCallId` (openai-completions.ts:1194-1218): extract
/// and normalize pipe-separated Responses-API ids, preserving item-level
/// uniqueness within the 40-char OpenAI limit.
fn normalize_tool_call_id(model: &Model, id: &str) -> String {
    if let Some(separator_index) = id.find('|') {
        let call_id = sanitize_tool_id_part(&id[..separator_index]);
        let item_id = sanitize_tool_id_part(&id[separator_index + 1..]);
        let combined_id = if item_id.is_empty() {
            call_id.clone()
        } else {
            format!("{call_id}_{item_id}")
        };
        if combined_id.len() <= 40 {
            return combined_id;
        }
        let hash: String = short_hash(id).chars().take(8).collect();
        let keep = (40usize)
            .saturating_sub(hash.chars().count())
            .saturating_sub(1)
            .max(1);
        let prefix: String = call_id.chars().take(keep).collect();
        return format!("{prefix}_{hash}");
    }
    if model.provider == "openai" {
        return id.chars().take(40).collect();
    }
    id.to_string()
}

// ---- message transformation (upstream api/transform-messages.ts) ----

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// Upstream `replaceImagesWithPlaceholder`
/// (`api/transform-messages.ts:15-33`): consecutive images collapse into one
/// placeholder.
fn replace_images_with_placeholder(
    content: &[TextOrImageBlock],
    placeholder: &str,
) -> Vec<TextOrImageBlock> {
    let mut result: Vec<TextOrImageBlock> = Vec::new();
    let mut previous_was_placeholder = false;
    for block in content {
        match block {
            TextOrImageBlock::Image(_) => {
                if !previous_was_placeholder {
                    result.push(TextOrImageBlock::Text(TextContent {
                        text: placeholder.to_string(),
                        text_signature: None,
                    }));
                }
                previous_was_placeholder = true;
            }
            other => {
                previous_was_placeholder =
                    matches!(other, TextOrImageBlock::Text(text) if text.text == placeholder);
                result.push(other.clone());
            }
        }
    }
    result
}

/// Upstream `transformMessages` (`api/transform-messages.ts:64-235`): image
/// downgrade, thinking-block transformation, tool-call id normalization, and
/// synthetic tool results for orphaned calls.
pub(crate) fn transform_messages(
    model: &Model,
    messages: &[Message],
    normalize_id: &dyn Fn(&str, &AssistantMessage) -> String,
) -> Vec<Message> {
    let image_aware: Vec<Message> = if model.input.contains(&ModelInput::Image) {
        messages.to_vec()
    } else {
        messages
            .iter()
            .map(|message| match message {
                Message::User(user) => match &user.content {
                    StringOrBlocks::Blocks(blocks) => Message::User(UserMessage {
                        content: StringOrBlocks::Blocks(replace_images_with_placeholder(
                            blocks,
                            NON_VISION_USER_IMAGE_PLACEHOLDER,
                        )),
                        timestamp: user.timestamp,
                    }),
                    _ => message.clone(),
                },
                Message::ToolResult(result) => Message::ToolResult(ToolResultMessage {
                    content: replace_images_with_placeholder(
                        &result.content,
                        NON_VISION_TOOL_IMAGE_PLACEHOLDER,
                    ),
                    ..result.clone()
                }),
                other => other.clone(),
            })
            .collect()
    };

    // First pass: transform assistant content, normalize cross-model tool ids.
    let mut tool_call_id_map: HashMap<String, String> = HashMap::new();
    let transformed: Vec<Message> = image_aware
        .iter()
        .map(|message| match message {
            Message::System(_) | Message::User(_) => message.clone(),
            Message::ToolResult(result) => match tool_call_id_map.get(&result.tool_call_id) {
                Some(normalized) if normalized != &result.tool_call_id => {
                    Message::ToolResult(ToolResultMessage {
                        tool_call_id: normalized.clone(),
                        ..result.clone()
                    })
                }
                _ => message.clone(),
            },
            Message::Assistant(assistant) => {
                let same_model = assistant.provider == model.provider
                    && assistant.api == model.api
                    && assistant.model == model.id;
                let content: Vec<AssistantBlock> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Thinking(thinking) => {
                            // Redacted thinking is opaque encrypted content,
                            // only valid for the same model.
                            if thinking.redacted == Some(true) {
                                same_model.then(|| block.clone())
                            } else if same_model && thinking.thinking_signature.is_some() {
                                // Keep signed blocks for replay even when the
                                // thinking text is empty (OpenAI encrypted
                                // reasoning).
                                Some(block.clone())
                            } else if thinking.thinking.trim().is_empty() {
                                None
                            } else if same_model {
                                Some(block.clone())
                            } else {
                                Some(AssistantBlock::Text(TextContent {
                                    text: thinking.thinking.clone(),
                                    text_signature: None,
                                }))
                            }
                        }
                        AssistantBlock::Text(text) => {
                            if same_model {
                                Some(block.clone())
                            } else {
                                Some(AssistantBlock::Text(TextContent {
                                    text: text.text.clone(),
                                    text_signature: None,
                                }))
                            }
                        }
                        AssistantBlock::ToolCall(call) => {
                            let mut call = call.clone();
                            if !same_model {
                                call.thought_signature = None;
                                // Upstream passes the source assistant message
                                // (transform-messages.ts:137) so per-API
                                // normalizers can tell cross-provider from
                                // same-provider-different-model ids.
                                let normalized = normalize_id(&call.id, assistant);
                                if normalized != call.id {
                                    tool_call_id_map.insert(call.id.clone(), normalized.clone());
                                    call.id = normalized;
                                }
                            }
                            Some(AssistantBlock::ToolCall(call))
                        }
                    })
                    .collect();
                Message::Assistant(AssistantMessage {
                    content,
                    ..assistant.clone()
                })
            }
        })
        .collect();

    // Second pass: insert synthetic empty tool results for orphaned calls.
    let mut result: Vec<Message> = Vec::new();
    let mut pending_tool_calls: Vec<ToolCall> = Vec::new();
    let mut existing_tool_result_ids: HashSet<String> = HashSet::new();
    let mut held_system_messages: Vec<Message> = Vec::new();

    // Upstream `closePendingToolCalls` (transform-messages.ts:167-186).
    fn close_pending(
        result: &mut Vec<Message>,
        pending: &mut Vec<ToolCall>,
        existing: &mut HashSet<String>,
        held: &mut Vec<Message>,
    ) {
        if !pending.is_empty() {
            for call in pending.iter() {
                if !existing.contains(&call.id) {
                    result.push(Message::ToolResult(ToolResultMessage {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        content: vec![TextOrImageBlock::Text(TextContent {
                            text: "No result provided".to_string(),
                            text_signature: None,
                        })],
                        details: None,
                        usage: None,
                        is_error: true,
                        timestamp: crate::ai::now_ms(),
                    }));
                }
            }
            pending.clear();
            existing.clear();
        }
        result.append(held);
    }

    for message in &transformed {
        match message {
            Message::Assistant(assistant) => {
                close_pending(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                    &mut held_system_messages,
                );
                // Skip errored/aborted assistants: incomplete turns that
                // should not be replayed.
                if assistant.stop_reason == StopReason::Error
                    || assistant.stop_reason == StopReason::Aborted
                {
                    continue;
                }
                let tool_calls: Vec<ToolCall> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::ToolCall(call) => Some(call.clone()),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    pending_tool_calls = tool_calls;
                    existing_tool_result_ids.clear();
                }
                result.push(message.clone());
            }
            Message::ToolResult(result_message) => {
                existing_tool_result_ids.insert(result_message.tool_call_id.clone());
                result.push(message.clone());
            }
            // System messages are transparent to tool-call accounting: hold
            // them while calls are pending so they never cause a duplicate
            // result.
            Message::System(_) => {
                if !pending_tool_calls.is_empty() {
                    held_system_messages.push(message.clone());
                } else {
                    result.push(message.clone());
                }
            }
            Message::User(_) => {
                close_pending(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                    &mut held_system_messages,
                );
                result.push(message.clone());
            }
        }
    }
    close_pending(
        &mut result,
        &mut pending_tool_calls,
        &mut existing_tool_result_ids,
        &mut held_system_messages,
    );
    result
}

// ---- message conversion (upstream convertMessages, openai-completions.ts:1185-1470) ----

const OPENAI_COMPLETIONS_REASONING_FIELDS: [&str; 3] =
    ["reasoning", "reasoning_content", "reasoning_text"];

const ASSISTANT_BRIDGE_TEXT: &str = "I have processed the tool results.";

/// Upstream `isOpenAIReasoningDetail` + `hasValidCommonReasoningDetailFields`
/// (openai-completions.ts:118-147). Shared with the streaming port, which
/// validates `reasoning_details` deltas with the same predicate.
pub(crate) fn is_openai_reasoning_detail(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let common_fields_valid = object
        .get("id")
        .is_none_or(|value| value.is_null() || value.is_string())
        && object.get("format").is_none_or(Value::is_string)
        && object.get("index").is_none_or(Value::is_number);
    if !common_fields_valid {
        return false;
    }
    match object.get("type").and_then(Value::as_str) {
        Some("reasoning.summary") => object.get("summary").is_some_and(Value::is_string),
        Some("reasoning.encrypted") => object.get("data").is_some_and(Value::is_string),
        Some("reasoning.text") => {
            object.get("text").is_some_and(Value::is_string)
                && object
                    .get("signature")
                    .is_none_or(|value| value.is_null() || value.is_string())
        }
        _ => false,
    }
}

/// Upstream `parseOpenAIReasoningDetails` (openai-completions.ts:215-223).
fn parse_openai_reasoning_details(signature: Option<&str>) -> Option<Value> {
    let parsed: Value = serde_json::from_str(signature?).ok()?;
    let array = parsed.as_array()?;
    if array.is_empty() || !array.iter().all(is_openai_reasoning_detail) {
        return None;
    }
    Some(parsed)
}

/// Upstream `parseLegacyEncryptedReasoningDetail`
/// (openai-completions.ts:225-241).
fn parse_legacy_encrypted_reasoning_detail(signature: Option<&str>) -> Option<Value> {
    let parsed: Value = serde_json::from_str(signature?).ok()?;
    if !is_openai_reasoning_detail(&parsed) {
        return None;
    }
    let object = parsed.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("reasoning.encrypted") {
        return None;
    }
    let id = object.get("id").and_then(Value::as_str)?;
    let data = object.get("data").and_then(Value::as_str)?;
    if id.is_empty() || data.is_empty() {
        return None;
    }
    Some(parsed)
}

/// Upstream `renderSystemMessageUpdate` (`utils/text.ts:28-40`): a later
/// system message framed by section name so the model can relate it to the
/// leading prompt.
pub(crate) fn render_system_message_update(message: &SystemMessage) -> String {
    let mut parts: Vec<String> = Vec::new();
    let text = content_text(&message.content);
    if !text.is_empty() {
        parts.push(text);
    }
    if let Some(sections) = &message.sections {
        for (name, value) in sections {
            parts.push(match value {
                Some(text) => format!("Updated system prompt section \"{name}\":\n\n{text}"),
                None => format!("Removed system prompt section \"{name}\"."),
            });
        }
    }
    parts.join("\n\n")
}

fn convert_messages(
    model: &Model,
    context: &TranscriptContext,
    compat: &OpenAiCompletionsCompat,
    grammar_tool_input_properties: &HashMap<String, String>,
) -> Result<Vec<Value>, String> {
    let normalized = resolve_transcript(context.clone(), compat.supports_mid_convo_system_messages);
    let transformed = transform_messages(model, normalized.messages(), &|id, _source| {
        normalize_tool_call_id(model, id)
    });
    let transcript_tools = resolve_transcript_tools(
        normalized.messages(),
        compat.supports_mid_convo_system_messages == Some(true)
            && compat.supports_mid_convo_tool_additions == Some(true),
    );
    let instruction_role = if model.reasoning && compat.supports_developer_role == Some(true) {
        "developer"
    } else {
        "system"
    };

    let mut params: Vec<Value> = Vec::new();
    let mut last_role: Option<&'static str> = None;
    let mut index = 0usize;
    while index < transformed.len() {
        let message = &transformed[index];
        // Some providers don't allow user messages directly after tool
        // results; insert a synthetic assistant message to bridge the gap.
        if compat.requires_assistant_after_tool_result == Some(true)
            && last_role == Some("toolResult")
            && matches!(message, Message::User(_))
        {
            params.push(json!({"role": "assistant", "content": ASSISTANT_BRIDGE_TEXT}));
        }

        match message {
            Message::System(system) => {
                let added_tools = if index > 0 && transcript_tools.anchors_additions {
                    system.tools_added.clone().unwrap_or_default()
                } else {
                    Vec::new()
                };
                if !added_tools.is_empty() {
                    // Kimi-style system message carrying in-place tool
                    // additions.
                    params.push(json!({
                        "role": "system",
                        "tools": convert_tools(&added_tools, compat)?
                    }));
                }
                let text = if index == 0 {
                    get_system_message_text(system)
                } else {
                    render_system_message_update(system)
                };
                if !text.is_empty() {
                    params.push(json!({"role": instruction_role, "content": text}));
                }
                last_role = Some("system");
                index += 1;
            }
            Message::User(user) => {
                match &user.content {
                    StringOrBlocks::Text(text) => {
                        params.push(json!({"role": "user", "content": text}));
                    }
                    StringOrBlocks::Blocks(blocks) => {
                        let content: Vec<Value> = blocks
                            .iter()
                            // Upstream (1b6ddca87): empty text parts are
                            // omitted from multimodal user messages.
                            .filter(|block| {
                                !matches!(block, TextOrImageBlock::Text(text) if text.text.is_empty())
                            })
                            .map(|block| match block {
                                TextOrImageBlock::Text(text) => {
                                    json!({"type": "text", "text": text.text})
                                }
                                TextOrImageBlock::Image(image) => json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{};base64,{}", image.mime_type, image.data)
                                    }
                                }),
                            })
                            .collect();
                        if content.is_empty() {
                            // Upstream `continue`: no message, no lastRole
                            // update.
                            index += 1;
                            continue;
                        }
                        params.push(json!({"role": "user", "content": content}));
                    }
                }
                last_role = Some("user");
                index += 1;
            }
            Message::Assistant(assistant) => {
                // Some providers don't accept null content, use empty string
                // instead when the bridge is required.
                let mut wire = Map::new();
                wire.insert("role".into(), json!("assistant"));
                wire.insert(
                    "content".into(),
                    if compat.requires_assistant_after_tool_result == Some(true) {
                        json!("")
                    } else {
                        Value::Null
                    },
                );

                let text_parts: Vec<Value> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Text(text) if !text.text.trim().is_empty() => {
                            Some(json!({"type": "text", "text": text.text}))
                        }
                        _ => None,
                    })
                    .collect();
                let assistant_text: String = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Text(text) if !text.text.trim().is_empty() => {
                            Some(text.text.as_str())
                        }
                        _ => None,
                    })
                    .collect();

                let thinking_blocks: Vec<&ThinkingContent> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Thinking(thinking) => Some(thinking),
                        _ => None,
                    })
                    .collect();
                let tool_calls: Vec<&ToolCall> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::ToolCall(call) => Some(call),
                        _ => None,
                    })
                    .collect();

                let signed_reasoning_details = thinking_blocks.iter().find_map(|block| {
                    parse_openai_reasoning_details(block.thinking_signature.as_deref())
                });
                let legacy_reasoning_details: Vec<Value> = tool_calls
                    .iter()
                    .filter_map(|call| {
                        parse_legacy_encrypted_reasoning_detail(call.thought_signature.as_deref())
                    })
                    .collect();
                let preserved_reasoning_details = signed_reasoning_details.or({
                    (!legacy_reasoning_details.is_empty())
                        .then_some(Value::Array(legacy_reasoning_details))
                });

                let non_empty_thinking: Vec<&&ThinkingContent> = thinking_blocks
                    .iter()
                    .filter(|block| !block.thinking.trim().is_empty())
                    .collect();
                if !non_empty_thinking.is_empty() {
                    if compat.requires_thinking_as_text == Some(true) {
                        // Convert thinking blocks to plain text (no tags to
                        // avoid model mimicking them).
                        let thinking_text = non_empty_thinking
                            .iter()
                            .map(|block| block.thinking.as_str())
                            .collect::<Vec<&str>>()
                            .join("\n\n");
                        let mut content = vec![json!({"type": "text", "text": thinking_text})];
                        content.extend(text_parts);
                        wire.insert("content".into(), Value::Array(content));
                    } else {
                        // Always send assistant content as a plain string
                        // (OpenAI Chat Completions standard format).
                        if !assistant_text.is_empty() {
                            wire.insert("content".into(), json!(assistant_text));
                        }
                        // reasoning_details is the structured alternative to
                        // a raw reasoning field.
                        if preserved_reasoning_details.is_none() {
                            // Use the signature from the first thinking block
                            // if available (llama.cpp server + gpt-oss).
                            let mut signature = non_empty_thinking[0].thinking_signature.clone();
                            if model.provider == "opencode-go"
                                && signature.as_deref() == Some("reasoning")
                            {
                                signature = Some("reasoning_content".to_string());
                            }
                            if let Some(signature) = &signature {
                                if OPENAI_COMPLETIONS_REASONING_FIELDS.contains(&signature.as_str())
                                {
                                    let joined = non_empty_thinking
                                        .iter()
                                        .map(|block| block.thinking.as_str())
                                        .collect::<Vec<&str>>()
                                        .join("\n");
                                    wire.insert(signature.clone(), json!(joined));
                                }
                            }
                        }
                    }
                } else if !assistant_text.is_empty() {
                    wire.insert("content".into(), json!(assistant_text));
                }

                if !tool_calls.is_empty() {
                    let mut wire_calls = Vec::with_capacity(tool_calls.len());
                    for call in &tool_calls {
                        if let Some(input_property) = grammar_tool_input_properties.get(&call.name)
                        {
                            let input = get_grammar_tool_input(
                                &call.name,
                                &call.arguments,
                                input_property,
                            )?;
                            wire_calls.push(json!({
                                "id": call.id,
                                "type": "custom",
                                "custom": {"name": call.name, "input": input}
                            }));
                        } else {
                            wire_calls.push(json!({
                                "id": call.id,
                                "type": "function",
                                "function": {"name": call.name, "arguments": call.arguments.to_string()}
                            }));
                        }
                    }
                    wire.insert("tool_calls".into(), Value::Array(wire_calls));
                }
                if let Some(details) = preserved_reasoning_details {
                    wire.insert("reasoning_details".into(), details);
                }
                if compat.requires_reasoning_content_on_assistant_messages == Some(true)
                    && model.reasoning
                    && !wire.contains_key("reasoning_content")
                {
                    wire.insert("reasoning_content".into(), json!(""));
                }

                // Skip assistant messages that have no content and no tool
                // calls (aborted responses that got no content).
                let has_content = match wire.get("content") {
                    Some(Value::String(text)) => !text.is_empty(),
                    Some(Value::Array(parts)) => !parts.is_empty(),
                    _ => false,
                };
                if !has_content && !wire.contains_key("tool_calls") {
                    index += 1;
                    continue;
                }
                params.push(Value::Object(wire));
                last_role = Some("assistant");
                index += 1;
            }
            Message::ToolResult(_) => {
                let mut image_blocks: Vec<Value> = Vec::new();
                let mut run_end = index;
                while run_end < transformed.len() {
                    let Message::ToolResult(tool_result) = &transformed[run_end] else {
                        break;
                    };
                    let text_result = tool_result
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            TextOrImageBlock::Text(text) => Some(text.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<&str>>()
                        .join("\n");
                    let has_images = tool_result
                        .content
                        .iter()
                        .any(|block| matches!(block, TextOrImageBlock::Image(_)));
                    // Always send tool result with text (or placeholder if
                    // only images).
                    let has_text = !text_result.is_empty();
                    let tool_result_text = if has_text {
                        text_result
                    } else if has_images {
                        "(see attached image)".to_string()
                    } else {
                        "(no tool output)".to_string()
                    };
                    let mut wire = json!({
                        "role": "tool",
                        "content": tool_result_text,
                        "tool_call_id": tool_result.tool_call_id
                    });
                    // Some providers require the 'name' field in tool results.
                    if compat.requires_tool_result_name == Some(true)
                        && !tool_result.tool_name.is_empty()
                    {
                        wire["name"] = json!(tool_result.tool_name);
                    }
                    params.push(wire);

                    if has_images && model.input.contains(&ModelInput::Image) {
                        for block in &tool_result.content {
                            if let TextOrImageBlock::Image(image) = block {
                                image_blocks.push(json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{};base64,{}", image.mime_type, image.data)
                                    }
                                }));
                            }
                        }
                    }
                    run_end += 1;
                }
                index = run_end;

                if !image_blocks.is_empty() {
                    if compat.requires_assistant_after_tool_result == Some(true) {
                        params.push(json!({"role": "assistant", "content": ASSISTANT_BRIDGE_TEXT}));
                    }
                    let mut content = vec![
                        json!({"type": "text", "text": "Attached image(s) from tool result:"}),
                    ];
                    content.extend(image_blocks);
                    params.push(json!({"role": "user", "content": content}));
                    last_role = Some("user");
                } else {
                    last_role = Some("toolResult");
                }
                continue;
            }
        }
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::super::compat_detect::{detect_openai_completions_compat_for_model, merge_compat};
    use super::*;
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::message::Sections;
    use crate::ai::types::primitives::{ToolChoice, UsageCost};
    use crate::ai::types::ModelInput;
    use serde_json::{json, Value};

    const TS: i64 = 1758240000000;

    // ---- fixtures ----

    fn make_model(
        provider: &str,
        base_url: &str,
        id: &str,
        reasoning: bool,
        compat: Value,
    ) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: id.to_string(),
            name: "Test Model".to_string(),
            api: "openai-completions".to_string(),
            provider: provider.to_string(),
            base_url: base_url.to_string(),
            reasoning,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: crate::ai::types::primitives::ModelCost::default(),
            context_window: 128000,
            max_tokens: 4096,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: Some(compat),
        }
    }

    fn resolved(model: &Model) -> OpenAiCompletionsCompat {
        merge_compat(
            detect_openai_completions_compat_for_model(&model.provider, &model.base_url, &model.id),
            model.compat.as_ref(),
        )
    }

    fn cfg(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: base_url.to_string(),
            api_key: "test-key".to_string(),
            max_tokens: 4096,
        }
    }

    fn opts() -> SimpleStreamOptions {
        SimpleStreamOptions::default()
    }

    fn ctx_of(messages: Vec<Message>) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: None,
            messages,
            tools: None,
        })
    }

    fn prompt_ctx(
        prompt: &str,
        messages: Vec<Message>,
        tools: Option<Vec<Tool>>,
    ) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: Some(prompt.to_string()),
            messages,
            tools,
        })
    }

    fn user(content: &str) -> Message {
        user_at(content, TS)
    }

    fn user_at(content: &str, ts: i64) -> Message {
        Message::User(UserMessage {
            content: StringOrBlocks::Text(content.to_string()),
            timestamp: ts,
        })
    }

    fn user_blocks(blocks: Vec<TextOrImageBlock>, ts: i64) -> Message {
        Message::User(UserMessage {
            content: StringOrBlocks::Blocks(blocks),
            timestamp: ts,
        })
    }

    fn text_block(text: &str) -> TextOrImageBlock {
        TextOrImageBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    }

    fn image_block() -> TextOrImageBlock {
        TextOrImageBlock::Image(crate::ai::types::content::ImageContent {
            data: "aGVsbG8=".to_string(),
            mime_type: "image/png".to_string(),
        })
    }

    fn empty_usage() -> Usage {
        Usage {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 0,
            cost: UsageCost::default(),
        }
    }

    fn assistant(
        provider: &str,
        api: &str,
        model_id: &str,
        content: Vec<AssistantBlock>,
        stop: StopReason,
    ) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            api: api.to_string(),
            provider: provider.to_string(),
            model: model_id.to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: empty_usage(),
            stop_reason: stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: TS + 1,
        })
    }

    fn thinking(text: &str, signature: Option<&str>) -> AssistantBlock {
        AssistantBlock::Thinking(ThinkingContent {
            thinking: text.to_string(),
            thinking_signature: signature.map(str::to_string),
            redacted: None,
        })
    }

    fn a_text(text: &str) -> AssistantBlock {
        AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    }

    fn tool_call(id: &str, name: &str, arguments: Value) -> AssistantBlock {
        AssistantBlock::ToolCall(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
            thought_signature: None,
            namespace: None,
        })
    }

    fn tool_result(call_id: &str, name: &str, content: Vec<TextOrImageBlock>) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: call_id.to_string(),
            tool_name: name.to_string(),
            content,
            details: None,
            usage: None,
            is_error: false,
            timestamp: TS + 2,
        })
    }

    fn sys(content: &str, ts: i64) -> Message {
        Message::System(SystemMessage {
            content: StringOrBlocks::Text(content.to_string()),
            sections: None,
            tools_added: None,
            tools_removed: None,
            timestamp: ts,
        })
    }

    fn sys_with_sections(content: &str, ts: i64, sections: Vec<(&str, Option<&str>)>) -> Message {
        Message::System(SystemMessage {
            content: StringOrBlocks::Text(content.to_string()),
            sections: Some(Sections::new(
                sections
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value.map(str::to_string)))
                    .collect(),
            )),
            tools_added: None,
            tools_removed: None,
            timestamp: ts,
        })
    }

    fn plain_tool(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: format!("{name} tool"),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            constrained_sampling: None,
        }
    }

    fn roles(body: &Value) -> Vec<&str> {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect()
    }

    fn build(
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> Result<Value, String> {
        build_request(model, &cfg(&model.base_url), ctx, options, &resolved(model))
            .map(|assembly| assembly.body)
    }

    // ---- 1. empty tools / defaults (upstream openai-completions-empty-tools.test.ts) ----

    #[test]
    fn empty_tools_omit_the_tools_key_and_send_defaults() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let body = build(&model, &ctx_of(vec![user("hi")]), &opts()).unwrap();
        assert!(!body.as_object().unwrap().contains_key("tools"), "{body}");
        assert_eq!(body["max_completion_tokens"], json!(4096));
        assert!(body.as_object().unwrap().get("max_tokens").is_none());
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
        assert_eq!(body["store"], json!(false));
        assert_eq!(body["model"], json!("gpt-4o-mini"));
        assert_eq!(roles(&body), ["user"]);
        assert_eq!(body["messages"][0]["content"], json!("hi"));
        // No sessionId: prompt-cache fields stay absent even for direct OpenAI.
        assert!(body.as_object().unwrap().get("prompt_cache_key").is_none());
        assert!(body
            .as_object()
            .unwrap()
            .get("prompt_cache_retention")
            .is_none());
    }

    #[test]
    fn explicit_max_tokens_uses_max_completion_tokens() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let mut options = opts();
        options.stream.max_tokens = Some(1234);
        let body = build(&model, &ctx_of(vec![user("hi")]), &options).unwrap();
        assert_eq!(body["max_completion_tokens"], json!(1234));
        assert!(body.as_object().unwrap().get("max_tokens").is_none());
    }

    #[test]
    fn max_tokens_clamps_to_remaining_context() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        model.context_window = 10000;
        model.max_tokens = 8000;
        let long_user = user_at(&"x".repeat(8000), TS);

        let body = build(&model, &ctx_of(vec![long_user.clone()]), &opts()).unwrap();
        assert_eq!(body["max_completion_tokens"], json!(3904));

        let mut options = opts();
        options.stream.max_tokens = Some(7000);
        let body = build(&model, &ctx_of(vec![long_user]), &options).unwrap();
        assert_eq!(body["max_completion_tokens"], json!(3904));
    }

    #[test]
    fn max_tokens_field_compat_uses_max_tokens() {
        let model = make_model(
            "custom-deepseek",
            "https://api.deepseek.com/v1",
            "m",
            false,
            json!({}),
        );
        let mut options = opts();
        options.stream.max_tokens = Some(123);
        let body = build(&model, &ctx_of(vec![user("hi")]), &options).unwrap();
        assert_eq!(body["max_tokens"], json!(123));
        assert!(body
            .as_object()
            .unwrap()
            .get("max_completion_tokens")
            .is_none());
    }

    #[test]
    fn stream_options_omitted_when_unsupported() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsUsageInStreaming": false}),
        );
        let body = build(&model, &ctx_of(vec![user("hi")]), &opts()).unwrap();
        assert!(body.as_object().unwrap().get("stream_options").is_none());
    }

    #[test]
    fn tool_history_with_no_request_tools_sends_empty_tools_array() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let context = normalize_context(&Context {
            system_prompt: None,
            messages: vec![
                user("use the tool"),
                assistant(
                    "openai",
                    "openai-completions",
                    "gpt-4o-mini",
                    vec![tool_call("t1", "noop", json!({}))],
                    StopReason::ToolUse,
                ),
                tool_result("t1", "noop", vec![text_block("done")]),
            ],
            tools: Some(vec![]),
        });
        let body = build(&model, &context, &opts()).unwrap();
        assert_eq!(body["tools"], json!([]));
    }

    #[test]
    fn cloudflare_gateway_uses_conservative_fields() {
        let model = make_model(
            "cloudflare-ai-gateway",
            "https://gateway.ai.cloudflare.com/v1/acct/gw/compat",
            "workers-ai/@cf/moonshotai/kimi-k2.6",
            true,
            json!({}),
        );
        let mut options = opts();
        options.stream.max_tokens = Some(1234);
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(
            &model,
            &prompt_ctx("You are helpful.", vec![user("hi")], None),
            &options,
        )
        .unwrap();
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(body["messages"][0]["content"], json!("You are helpful."));
        assert_eq!(body["max_tokens"], json!(1234));
        assert!(body
            .as_object()
            .unwrap()
            .get("max_completion_tokens")
            .is_none());
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());
        assert!(body.as_object().unwrap().get("store").is_none());
    }

    // ---- 2. system/developer roles + mid-convo folding ----

    #[test]
    fn developer_role_for_reasoning_models_with_support() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-5.5",
            true,
            json!({}),
        );
        let body = build(
            &model,
            &prompt_ctx("Follow instructions.", vec![user("hi")], None),
            &opts(),
        )
        .unwrap();
        assert_eq!(body["messages"][0]["role"], json!("developer"));
        assert_eq!(
            body["messages"][0]["content"],
            json!("Follow instructions.")
        );
    }

    #[test]
    fn system_role_for_non_reasoning_models() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let body = build(
            &model,
            &prompt_ctx("Follow instructions.", vec![user("hi")], None),
            &opts(),
        )
        .unwrap();
        assert_eq!(body["messages"][0]["role"], json!("system"));
    }

    #[test]
    fn mid_convo_system_messages_fold_when_unsupported() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let messages = vec![
            sys_with_sections("base", 10, vec![("a", Some("<a>1</a>"))]),
            user("hello"),
            sys("also do this", 12),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![a_text("ok")],
                StopReason::Stop,
            ),
            sys_with_sections(
                "",
                14,
                vec![
                    ("a", Some("<a>2</a>")),
                    ("b", None),
                    ("c", Some("<c>1</c>")),
                ],
            ),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(roles(&body), ["system", "user", "assistant"], "{body}");
        assert_eq!(
            body["messages"][0]["content"],
            json!("base\n\nalso do this\n\n<a>2</a>\n\n<c>1</c>")
        );
    }

    #[test]
    fn mid_convo_system_messages_render_updates_when_supported() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsMidConvoSystemMessages": true}),
        );
        let messages = vec![
            sys_with_sections("base", 10, vec![("a", Some("<a>1</a>"))]),
            user("hello"),
            sys("also do this", 12),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![a_text("ok")],
                StopReason::Stop,
            ),
            sys_with_sections(
                "",
                14,
                vec![
                    ("a", Some("<a>2</a>")),
                    ("b", None),
                    ("c", Some("<c>1</c>")),
                ],
            ),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(
            roles(&body),
            ["system", "user", "system", "assistant", "system"]
        );
        assert_eq!(body["messages"][0]["content"], json!("base\n\n<a>1</a>"));
        assert_eq!(body["messages"][2]["content"], json!("also do this"));
        assert_eq!(
            body["messages"][4]["content"],
            json!(
                "Updated system prompt section \"a\":\n\n<a>2</a>\n\nRemoved system prompt section \"b\".\n\nUpdated system prompt section \"c\":\n\n<c>1</c>"
            )
        );
    }

    // ---- 3. user content ----

    #[test]
    fn user_text_and_image_parts_replay() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        model.input = vec![ModelInput::Text, ModelInput::Image];
        let messages = vec![
            user_blocks(vec![text_block("what is this?"), image_block()], TS),
            user("hi"),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(roles(&body), ["user", "user"]);
        assert_eq!(
            body["messages"][0]["content"],
            json!([
                {"type":"text","text":"what is this?"},
                {"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8="}}
            ])
        );
        assert_eq!(body["messages"][1]["content"], json!("hi"));
    }

    #[test]
    fn empty_user_block_content_is_skipped() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let messages = vec![user_blocks(vec![], TS), user("hi")];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(roles(&body), ["user"]);
    }

    // ---- 4. assistant replay ----

    #[test]
    fn assistant_replays_tool_calls_with_json_string_arguments() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let messages = vec![
            user("Running?"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![
                    a_text("Running ls."),
                    tool_call("call_1", "bash", json!({"command":"ls"})),
                ],
                StopReason::ToolUse,
            ),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(
            body["messages"][1],
            json!({
                "role": "assistant",
                "content": "Running ls.",
                "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "bash", "arguments": "{\"command\":\"ls\"}"}}
                ]
            })
        );
    }

    #[test]
    fn assistant_without_content_or_tool_calls_is_skipped() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let messages = vec![
            user("hi"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![],
                StopReason::Stop,
            ),
            user("ho"),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(roles(&body), ["user", "user"]);
    }

    #[test]
    fn reasoning_content_replays_from_thinking_signature() {
        let model = make_model(
            "zai",
            "https://api.z.ai/api/paas/v4",
            "glm-5.2",
            true,
            json!({}),
        );
        let messages = vec![
            user("Read README.md"),
            assistant(
                "zai",
                "openai-completions",
                "glm-5.2",
                vec![
                    thinking("prior reasoning", Some("reasoning_content")),
                    tool_call("call_1", "read", json!({"path":"README.md"})),
                ],
                StopReason::ToolUse,
            ),
            tool_result("call_1", "read", vec![text_block("contents")]),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        let assistant_value = &body["messages"][1];
        assert_eq!(assistant_value["role"], json!("assistant"));
        assert_eq!(
            assistant_value["reasoning_content"],
            json!("prior reasoning")
        );
        assert!(assistant_value
            .as_object()
            .unwrap()
            .get("reasoning")
            .is_none());
        assert_eq!(assistant_value["content"], Value::Null);
    }

    #[test]
    fn opencode_go_reasoning_signature_normalizes_to_reasoning_content() {
        let model = make_model(
            "opencode-go",
            "https://opencode.ai/zen/v1",
            "kimi-k2.6",
            true,
            json!({}),
        );
        let messages = vec![assistant(
            "opencode-go",
            "openai-completions",
            "kimi-k2.6",
            vec![
                thinking("think", Some("reasoning")),
                tool_call("call_1", "read", json!({"path":"README.md"})),
            ],
            StopReason::Stop,
        )];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(body["messages"][0]["reasoning_content"], json!("think"));
        assert!(body["messages"][0]
            .as_object()
            .unwrap()
            .get("reasoning")
            .is_none());
    }

    #[test]
    fn requires_reasoning_content_adds_empty_field() {
        let model = make_model(
            "xiaomi",
            "https://api.xiaomi.com/v1",
            "mimo-v2.5-pro",
            true,
            json!({"requiresReasoningContentOnAssistantMessages": true, "thinkingFormat": "deepseek"}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let messages = vec![
            user("Read README.md"),
            assistant(
                "xiaomi",
                "openai-completions",
                "mimo-v2.5-pro",
                vec![tool_call("call_1", "read", json!({"path":"README.md"}))],
                StopReason::ToolUse,
            ),
            tool_result("call_1", "read", vec![text_block("contents")]),
        ];
        let body = build(&model, &ctx_of(messages), &options).unwrap();
        let assistant_value = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == json!("assistant"))
            .unwrap();
        assert_eq!(assistant_value["reasoning_content"], json!(""));
        // deepseek thinking format with reasoning on.
        assert_eq!(body["thinking"], json!({"type": "enabled"}));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn reasoning_details_replay_from_signature() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            true,
            json!({}),
        );
        let messages = vec![assistant(
            "openai",
            "openai-completions",
            "gpt-4o-mini",
            vec![
                thinking("", Some(r#"[{"type":"reasoning.encrypted","data":"abc"}]"#)),
                tool_call("call_1", "read", json!({"path":"README.md"})),
            ],
            StopReason::ToolUse,
        )];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        let assistant_value = &body["messages"][0];
        assert_eq!(
            assistant_value["reasoning_details"],
            json!([{"type":"reasoning.encrypted","data":"abc"}])
        );
        assert!(assistant_value
            .as_object()
            .unwrap()
            .get("reasoning_content")
            .is_none());
        assert_eq!(assistant_value["content"], Value::Null);
    }

    /// The tool-call `thoughtSignature` replay: a tool call carrying the
    /// legacy encrypted reasoning detail (upstream
    /// `parseLegacyEncryptedReasoningDetail` over `call.thoughtSignature`)
    /// re-emits `reasoning_details` on the assistant wire message.
    #[test]
    fn legacy_thought_signature_replays_as_reasoning_details() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            true,
            json!({}),
        );
        let mut call = tool_call("call_1", "read", json!({"path":"README.md"}));
        match &mut call {
            AssistantBlock::ToolCall(call) => {
                call.thought_signature =
                    Some(r#"{"type":"reasoning.encrypted","id":"call_1","data":"enc"}"#.into());
            }
            other => panic!("expected a tool call, got {other:?}"),
        }
        let messages = vec![assistant(
            "openai",
            "openai-completions",
            "gpt-4o-mini",
            vec![call],
            StopReason::ToolUse,
        )];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        let assistant_value = &body["messages"][0];
        assert_eq!(
            assistant_value["reasoning_details"],
            json!([{"type":"reasoning.encrypted","id":"call_1","data":"enc"}])
        );
        // The replayed details also suppress the reasoning-field fallback.
        assert!(assistant_value
            .as_object()
            .unwrap()
            .get("reasoning_content")
            .is_none());
    }

    // ---- 5. thinking as text ----

    #[test]
    fn thinking_as_text_serializes_as_plain_text_parts() {
        let model = make_model(
            "repro-provider",
            "http://127.0.0.1:1",
            "repro-model",
            true,
            json!({"requiresThinkingAsText": true}),
        );
        // Upstream openai-completions-thinking-as-text.test.ts pins plain text
        // parts (no tags): the compat comment says "<thinking>-delimited" but
        // the implementation joins without tags to avoid model mimicking.
        let messages = vec![
            user("hello"),
            assistant(
                "repro-provider",
                "openai-completions",
                "repro-model",
                vec![
                    thinking("internal reasoning", None),
                    a_text("visible answer"),
                ],
                StopReason::Stop,
            ),
            user("continue"),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(
            body["messages"][1],
            json!({
                "role": "assistant",
                "content": [
                    {"type":"text","text":"internal reasoning"},
                    {"type":"text","text":"visible answer"}
                ]
            })
        );

        let messages = vec![assistant(
            "repro-provider",
            "openai-completions",
            "repro-model",
            vec![thinking("internal reasoning", None)],
            StopReason::Stop,
        )];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(
            body["messages"][0]["content"],
            json!([{"type":"text","text":"internal reasoning"}])
        );
    }

    // ---- 6. tool results ----

    #[test]
    fn tool_results_replay_with_ids_and_optional_name() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let messages = vec![
            user("q"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![tool_call("call_1", "read", json!({"path":"a.md"}))],
                StopReason::ToolUse,
            ),
            tool_result(
                "call_1",
                "read",
                vec![text_block("line one"), text_block("line two")],
            ),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(
            body["messages"][2],
            json!({"role":"tool","content":"line one\nline two","tool_call_id":"call_1"})
        );

        // requiresToolResultName adds the tool name.
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"requiresToolResultName": true}),
        );
        let messages = vec![
            user("q"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![tool_call("call_1", "read", json!({}))],
                StopReason::ToolUse,
            ),
            tool_result("call_1", "read", vec![text_block("contents")]),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(
            body["messages"][2],
            json!({"role":"tool","content":"contents","tool_call_id":"call_1","name":"read"})
        );
    }

    #[test]
    fn requires_assistant_after_tool_result_inserts_bridge() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"requiresAssistantAfterToolResult": true}),
        );
        let messages = vec![
            user("q"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![tool_call("call_1", "read", json!({}))],
                StopReason::ToolUse,
            ),
            tool_result("call_1", "read", vec![text_block("contents")]),
            user("Continue"),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(
            roles(&body),
            ["user", "assistant", "tool", "assistant", "user"]
        );
        assert_eq!(
            body["messages"][3]["content"],
            json!("I have processed the tool results.")
        );
    }

    #[test]
    fn tool_result_images_batch_after_consecutive_results() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        model.input = vec![ModelInput::Text, ModelInput::Image];
        let messages = vec![
            user("Read the images"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![
                    tool_call("tool-1", "read", json!({"path":"img-1.png"})),
                    tool_call("tool-2", "read", json!({"path":"img-2.png"})),
                ],
                StopReason::ToolUse,
            ),
            tool_result(
                "tool-1",
                "read",
                vec![text_block("Read image file [image/png]"), image_block()],
            ),
            tool_result(
                "tool-2",
                "read",
                vec![text_block("Read image file [image/png]"), image_block()],
            ),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(roles(&body), ["user", "assistant", "tool", "tool", "user"]);
        let image_message = &body["messages"][4];
        let image_parts = image_message["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["type"] == json!("image_url"))
            .count();
        assert_eq!(image_parts, 2);
        assert_eq!(
            image_message["content"][0],
            json!({"type":"text","text":"Attached image(s) from tool result:"})
        );
    }

    #[test]
    fn empty_and_image_only_tool_results_use_placeholders() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        model.input = vec![ModelInput::Text, ModelInput::Image];
        // Empty text, no images: "(no tool output)".
        let messages = vec![
            user("Run the command"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![tool_call("tool-1", "bash", json!({"command":"true"}))],
                StopReason::ToolUse,
            ),
            tool_result("tool-1", "bash", vec![text_block("")]),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(body["messages"][2]["content"], json!("(no tool output)"));

        // Images only: text placeholder, then the batched image user message.
        let messages = vec![
            user("Read the image"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![tool_call("tool-1", "read", json!({}))],
                StopReason::ToolUse,
            ),
            tool_result("tool-1", "read", vec![image_block()]),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(
            body["messages"][2]["content"],
            json!("(see attached image)")
        );
        assert_eq!(roles(&body), ["user", "assistant", "tool", "user"]);
    }

    #[test]
    fn non_vision_model_downgrades_images_to_placeholders() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let messages = vec![
            user_blocks(
                vec![text_block("before"), image_block(), text_block("after")],
                TS,
            ),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![tool_call("call_1", "read", json!({}))],
                StopReason::ToolUse,
            ),
            tool_result("call_1", "read", vec![image_block()]),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(
            body["messages"][0]["content"],
            json!([
                {"type":"text","text":"before"},
                {"type":"text","text":"(image omitted: model does not support images)"},
                {"type":"text","text":"after"}
            ])
        );
        assert_eq!(
            body["messages"][2]["content"],
            json!("(tool image omitted: model does not support images)")
        );
    }

    // ---- 7. cross-model transforms ----

    #[test]
    fn cross_model_thinking_becomes_text_and_ids_normalize() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let messages = vec![
            user("q"),
            assistant(
                "anthropic",
                "anthropic-messages",
                "claude-sonnet-4-5",
                vec![
                    thinking("hmm", None),
                    tool_call("abc123|ws/item|+3d==", "read", json!({"path":"README.md"})),
                ],
                StopReason::ToolUse,
            ),
            tool_result("abc123|ws/item|+3d==", "read", vec![text_block("contents")]),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(body["messages"][1]["content"], json!("hmm"));
        assert_eq!(
            body["messages"][1]["tool_calls"][0]["id"],
            json!("abc123_ws_item__3d__")
        );
        assert_eq!(
            body["messages"][2]["tool_call_id"],
            json!("abc123_ws_item__3d__")
        );
    }

    #[test]
    fn orphaned_tool_calls_get_synthetic_results_and_errored_assistants_are_skipped() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let messages = vec![
            user("q"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![tool_call("t1", "noop", json!({}))],
                StopReason::ToolUse,
            ),
            user("next"),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(roles(&body), ["user", "assistant", "tool", "user"]);
        assert_eq!(body["messages"][2]["content"], json!("No result provided"));
        assert_eq!(body["messages"][2]["tool_call_id"], json!("t1"));

        let messages = vec![
            user("q"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![a_text("partial")],
                StopReason::Error,
            ),
            user("next"),
        ];
        let body = build(&model, &ctx_of(messages), &opts()).unwrap();
        assert_eq!(roles(&body), ["user", "user"]);
    }

    // ---- 8. tools: strict + grammar ----

    #[test]
    fn plain_tools_carry_strict_false_and_raw_parameters() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            // Upstream 890f92088: strict is only carried when the model
            // explicitly opts in (compat supportsStrictMode); the detected
            // default is now false, and a non-capable model omits the field
            // entirely (see strict_mode_disabled_omits_the_strict_field).
            json!({"supportsStrictMode": true}),
        );
        let body = build(
            &model,
            &prompt_ctx("sys", vec![user("hi")], Some(vec![plain_tool("read")])),
            &opts(),
        )
        .unwrap();
        assert_eq!(
            body["tools"][0],
            json!({
                "type": "function",
                "function": {
                    "name": "read",
                    "description": "read tool",
                    "parameters": {"type":"object","properties":{"path":{"type":"string"}}},
                    "strict": false
                }
            })
        );
    }

    #[test]
    fn strict_mode_disabled_omits_the_strict_field() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsStrictMode": false}),
        );
        let body = build(
            &model,
            &prompt_ctx("sys", vec![user("hi")], Some(vec![plain_tool("read")])),
            &opts(),
        )
        .unwrap();
        let function = &body["tools"][0]["function"];
        assert!(function.as_object().unwrap().get("strict").is_none());
    }

    #[test]
    fn json_schema_constrained_sampling_strictifies_parameters() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsStrictMode": true}),
        );
        let tool = Tool {
            name: "ping".to_string(),
            description: "Ping tool".to_string(),
            parameters: json!({"type":"object","properties":{"ok":{"type":"boolean"}}}),
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(
                crate::ai::types::tool::JsonSchemaSampling {
                    strict: Strict::Prefer,
                },
            )),
        };
        let body = build(
            &model,
            &prompt_ctx("sys", vec![user("hi")], Some(vec![tool])),
            &opts(),
        )
        .unwrap();
        assert_eq!(body["tools"][0]["function"]["strict"], json!(true));
        assert_eq!(
            body["tools"][0]["function"]["parameters"],
            json!({
                "type":"object",
                "properties":{"ok":{"anyOf":[{"type":"boolean"},{"type":"null"}]}},
                "required":["ok"],
                "additionalProperties":false
            })
        );
    }

    #[test]
    fn required_strict_sampling_on_unsupported_schema_errors() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsStrictMode": true}),
        );
        let tool = Tool {
            name: "ref_tool".to_string(),
            description: "Ref tool".to_string(),
            parameters: json!({"$ref":"#/x"}),
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(
                crate::ai::types::tool::JsonSchemaSampling {
                    strict: Strict::Require,
                },
            )),
        };
        let error = build(
            &model,
            &prompt_ctx("sys", vec![user("hi")], Some(vec![tool])),
            &opts(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Tool \"ref_tool\" requires JSON-schema constrained sampling, but $ref schemas are unsupported."
        );
    }

    #[test]
    fn required_strict_sampling_on_strict_unsupported_errors() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsStrictMode": false}),
        );
        let tool = Tool {
            name: "strict_tool".to_string(),
            description: "Strict tool".to_string(),
            parameters: json!({"type":"object","properties":{"ok":{"type":"boolean"}}}),
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(
                crate::ai::types::tool::JsonSchemaSampling {
                    strict: Strict::Require,
                },
            )),
        };
        let error = build(
            &model,
            &prompt_ctx("sys", vec![user("hi")], Some(vec![tool])),
            &opts(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Tool \"strict_tool\" requires JSON-schema constrained sampling, but strict tools are unsupported."
        );
    }

    #[test]
    fn grammar_tools_fall_back_to_the_regex_variant() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsOpenAIGrammarTools": true}),
        );
        let mut variants = BTreeMap::new();
        variants.insert(GrammarFormat::Regex, "[a-z]+".to_string());
        let tool = Tool {
            name: "exec".to_string(),
            description: "Run code".to_string(),
            parameters: json!({"type":"object","required":["input"],"properties":{"input":{"type":"string"}}}),
            constrained_sampling: Some(ConstrainedSampling::Grammar(GrammarSampling { variants })),
        };
        let body = build(
            &model,
            &prompt_ctx("sys", vec![user("hi")], Some(vec![tool])),
            &opts(),
        )
        .unwrap();
        assert_eq!(
            body["tools"][0]["custom"]["format"]["grammar"],
            json!({"syntax": "regex", "definition": "[a-z]+"})
        );
    }

    #[test]
    fn grammar_tools_emit_custom_defs_and_replay_custom_calls() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsOpenAIGrammarTools": true}),
        );
        let mut variants = BTreeMap::new();
        variants.insert(GrammarFormat::Lark, "start: WORD".to_string());
        let tool = Tool {
            name: "exec".to_string(),
            description: "Run code".to_string(),
            parameters: json!({"type":"object","required":["input"],"properties":{"input":{"type":"string"}}}),
            constrained_sampling: Some(ConstrainedSampling::Grammar(GrammarSampling { variants })),
        };
        let mut options = opts();
        let _ = &mut options;
        let messages = vec![
            user("Run?"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![tool_call("call_1", "exec", json!({"input":"abc"}))],
                StopReason::ToolUse,
            ),
        ];
        let body = build(
            &model,
            &prompt_ctx("sys", messages, Some(vec![tool])),
            &options,
        )
        .unwrap();
        assert_eq!(
            body["tools"][0],
            json!({
                "type": "custom",
                "custom": {
                    "name": "exec",
                    "description": "Run code",
                    "format": {"type": "grammar", "grammar": {"syntax": "lark", "definition": "start: WORD"}}
                }
            })
        );
        let assistant_value = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == json!("assistant"))
            .unwrap();
        assert_eq!(
            assistant_value["tool_calls"][0],
            json!({"id":"call_1","type":"custom","custom":{"name":"exec","input":"abc"}})
        );
    }

    /// Grammar error paths: a grammar tool with no usable variant (both
    /// encodings missing or whitespace-only) fails request assembly, and a
    /// replayed grammar tool call whose input property is not a string fails
    /// with the upstream message.
    #[test]
    fn grammar_tool_without_a_usable_variant_errors() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsOpenAIGrammarTools": true}),
        );
        for variants in [
            BTreeMap::new(),
            BTreeMap::from([
                (GrammarFormat::Lark, "   ".to_string()),
                (GrammarFormat::Regex, String::new()),
            ]),
        ] {
            let tool = Tool {
                name: "exec".to_string(),
                description: "Run code".to_string(),
                parameters: json!({"type":"object","required":["input"],"properties":{"input":{"type":"string"}}}),
                constrained_sampling: Some(ConstrainedSampling::Grammar(GrammarSampling {
                    variants,
                })),
            };
            let error = build(
                &model,
                &prompt_ctx("sys", vec![user("hi")], Some(vec![tool])),
                &opts(),
            )
            .unwrap_err();
            assert_eq!(
                error,
                "Tool \"exec\" cannot use grammar constrained sampling: no supported grammar variant was provided."
            );
        }
    }

    #[test]
    fn grammar_tool_call_with_a_non_string_input_errors() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({"supportsOpenAIGrammarTools": true}),
        );
        let mut variants = BTreeMap::new();
        variants.insert(GrammarFormat::Lark, "start: WORD".to_string());
        let tool = Tool {
            name: "exec".to_string(),
            description: "Run code".to_string(),
            parameters: json!({"type":"object","required":["input"],"properties":{"input":{"type":"string"}}}),
            constrained_sampling: Some(ConstrainedSampling::Grammar(GrammarSampling { variants })),
        };
        let messages = vec![
            user("Run?"),
            assistant(
                "openai",
                "openai-completions",
                "gpt-4o-mini",
                vec![tool_call("call_1", "exec", json!({"input": 42}))],
                StopReason::ToolUse,
            ),
        ];
        let error = build(
            &model,
            &prompt_ctx("sys", messages, Some(vec![tool])),
            &opts(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Grammar tool call \"exec\" requires argument \"input\" to be a string."
        );
    }

    // ---- 9. sampling params / temperature ----

    #[test]
    fn sampling_params_merge_model_then_options() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let mut model_params = BTreeMap::new();
        model_params.insert("top_p".to_string(), json!(0.9));
        model_params.insert("keep".to_string(), json!(1));
        model.sampling_params = Some(model_params);

        let mut options = opts();
        let mut option_params = BTreeMap::new();
        option_params.insert("top_p".to_string(), json!(0.5));
        option_params.insert("extra".to_string(), json!(true));
        options.stream.sampling_params = Some(option_params);

        let body = build(&model, &ctx_of(vec![user("hi")]), &options).unwrap();
        assert_eq!(body["top_p"], json!(0.5));
        assert_eq!(body["keep"], json!(1));
        assert_eq!(body["extra"], json!(true));
    }

    #[test]
    fn sampling_params_override_named_request_fields() {
        let model = make_model(
            "custom-deepseek",
            "https://api.deepseek.com/v1",
            "m",
            false,
            json!({}),
        );
        let mut options = opts();
        options.stream.max_tokens = Some(123);
        let mut option_params = BTreeMap::new();
        option_params.insert("max_tokens".to_string(), json!(999));
        options.stream.sampling_params = Some(option_params);
        let body = build(&model, &ctx_of(vec![user("hi")]), &options).unwrap();
        assert_eq!(body["max_tokens"], json!(999));
    }

    // Per-thinking-level sampling parameters (upstream
    // sampling-options.test.ts @ 200387122).

    /// Upstream "applies sampling params for the effective thinking level
    /// over model defaults": the requested level (`low`) is unsupported
    /// (`thinkingLevelMap` marks it `null`), so the resolver clamps to
    /// `high` and applies that level's overrides over the model defaults.
    #[test]
    fn sampling_params_by_thinking_level_apply_over_model_defaults() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            true,
            json!({}),
        );
        model.thinking_level_map = Some(BTreeMap::from([
            (String::from("low"), None),
            (String::from("medium"), None),
        ]));
        model.sampling_params = Some(BTreeMap::from([
            (String::from("temperature"), json!(1)),
            (String::from("top_p"), json!(0.95)),
        ]));
        model.sampling_params_by_thinking_level = Some(BTreeMap::from([(
            String::from("high"),
            BTreeMap::from([
                (String::from("temperature"), json!(0.8)),
                (String::from("top_k"), json!(64)),
            ]),
        )]));

        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Low);
        let body = build(&model, &ctx_of(vec![user("hi")]), &options).unwrap();
        assert_eq!(body["temperature"], json!(0.8));
        assert_eq!(body["top_p"], json!(0.95));
        assert_eq!(body["top_k"], json!(64));
    }

    /// Upstream "applies off sampling params when reasoning is disabled": a
    /// non-reasoning model clamps every level to `off`, whose overrides still
    /// apply.
    #[test]
    fn sampling_params_off_level_applies_when_reasoning_disabled() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        model.sampling_params_by_thinking_level = Some(BTreeMap::from([(
            String::from("off"),
            BTreeMap::from([(String::from("temperature"), json!(0.7))]),
        )]));

        let body = build(&model, &ctx_of(vec![user("hi")]), &opts()).unwrap();
        assert_eq!(body["temperature"], json!(0.7));
    }

    /// Upstream "merges stream-option keys over thinking-level keys".
    #[test]
    fn sampling_params_request_keys_override_level_keys() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            true,
            json!({}),
        );
        model.sampling_params_by_thinking_level = Some(BTreeMap::from([(
            String::from("low"),
            BTreeMap::from([
                (String::from("temperature"), json!(0.6)),
                (String::from("top_p"), json!(0.95)),
            ]),
        )]));

        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Low);
        options.stream.sampling_params =
            Some(BTreeMap::from([(String::from("top_p"), json!(0.5))]));
        let body = build(&model, &ctx_of(vec![user("hi")]), &options).unwrap();
        assert_eq!(body["temperature"], json!(0.6));
        assert_eq!(body["top_p"], json!(0.5));
    }

    /// The resolver grid itself (`resolveSamplingParams`): presence
    /// short-circuit, per-level lookup with clamping, and the merge order
    /// model → level → request.
    #[test]
    fn resolve_sampling_params_grid() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            true,
            json!({}),
        );
        model.thinking_level_map = Some(BTreeMap::from([
            (String::from("low"), None),
            (String::from("medium"), None),
        ]));
        model.sampling_params = Some(BTreeMap::from([(String::from("top_p"), json!(0.95))]));
        model.sampling_params_by_thinking_level = Some(BTreeMap::from([
            (
                String::from("high"),
                BTreeMap::from([
                    (String::from("temperature"), json!(0.8)),
                    (String::from("top_k"), json!(64)),
                ]),
            ),
            (
                String::from("off"),
                BTreeMap::from([(String::from("temperature"), json!(0.7))]),
            ),
        ]));
        let request = BTreeMap::from([(String::from("top_k"), json!(1))]);

        let bare = {
            let mut bare = model.clone();
            bare.sampling_params = None;
            bare.sampling_params_by_thinking_level = None;
            bare
        };
        // All three sources absent: `undefined`.
        assert_eq!(resolve_sampling_params(&bare, None, None), None);

        // Model defaults only (no override for the effective level).
        let mut no_levels = model.clone();
        no_levels.sampling_params_by_thinking_level = None;
        assert_eq!(
            resolve_sampling_params(&no_levels, Some(ThinkingLevel::Low), None),
            Some(BTreeMap::from([(String::from("top_p"), json!(0.95))]))
        );

        // `low` is unsupported: clamped to `high`, whose overrides win over
        // the model defaults and lose to the request keys.
        assert_eq!(
            resolve_sampling_params(&model, Some(ThinkingLevel::Low), Some(&request)),
            Some(BTreeMap::from([
                (String::from("top_p"), json!(0.95)),
                (String::from("temperature"), json!(0.8)),
                (String::from("top_k"), json!(1)),
            ]))
        );

        // `off` (None) on a reasoning model with `off` overrides.
        assert_eq!(
            resolve_sampling_params(&model, None, None),
            Some(BTreeMap::from([
                (String::from("top_p"), json!(0.95)),
                (String::from("temperature"), json!(0.7)),
            ]))
        );

        // A non-reasoning model clamps every level to `off`.
        let mut non_reasoning = model.clone();
        non_reasoning.reasoning = false;
        assert_eq!(
            resolve_sampling_params(&non_reasoning, Some(ThinkingLevel::Xhigh), None),
            Some(BTreeMap::from([
                (String::from("top_p"), json!(0.95)),
                (String::from("temperature"), json!(0.7)),
            ]))
        );

        // Request keys alone.
        assert_eq!(
            resolve_sampling_params(&bare, None, Some(&request)),
            Some(BTreeMap::from([(String::from("top_k"), json!(1))]))
        );
    }

    #[test]
    fn temperature_passthrough() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let mut options = opts();
        options.stream.temperature = Some(0.7);
        let body = build(&model, &ctx_of(vec![user("hi")]), &options).unwrap();
        assert_eq!(body["temperature"], json!(0.7));

        let body = build(&model, &ctx_of(vec![user("hi")]), &opts()).unwrap();
        assert!(body.as_object().unwrap().get("temperature").is_none());
    }

    // ---- 10. thinking token budget ----

    fn vllm_model(compat: Value) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "zai-org/glm-5.2".to_string(),
            name: "GLM 5.2 (local vLLM)".to_string(),
            api: "openai-completions".to_string(),
            provider: "local-vllm".to_string(),
            base_url: "http://localhost:8000/v1".to_string(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: crate::ai::types::primitives::ModelCost::default(),
            context_window: 262144,
            max_tokens: 16384,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: Some(compat),
        }
    }

    #[test]
    fn thinking_token_budget_follows_the_thinking_budgets() {
        // Alias field via supportsThinkingTokenBudget.
        let model = vllm_model(json!({"thinkingFormat":"zai","supportsThinkingTokenBudget":true}));
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Medium);
        options.thinking_budgets = Some(ThinkingBudgets {
            medium: Some(4096),
            ..Default::default()
        });
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["thinking_token_budget"], json!(4096));

        // No field, no alias: absent.
        let model = vllm_model(json!({"thinkingFormat":"zai"}));
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        for field in [
            "thinking_token_budget",
            "thinking_budget",
            "thinking_budget_tokens",
        ] {
            assert!(
                body.as_object().unwrap().get(field).is_none(),
                "{field}: {body}"
            );
        }

        // Thinking off: absent.
        let model = vllm_model(json!({"thinkingFormat":"zai","supportsThinkingTokenBudget":true}));
        let mut off = opts();
        off.thinking_budgets = Some(ThinkingBudgets {
            high: Some(8192),
            ..Default::default()
        });
        let body = build(&model, &ctx_of(vec![user("Hi")]), &off).unwrap();
        assert!(body
            .as_object()
            .unwrap()
            .get("thinking_token_budget")
            .is_none());
    }

    #[test]
    fn thinking_token_budget_clamps_xhigh_max_and_ceiling() {
        let model = vllm_model(json!({"thinkingFormat":"zai","supportsThinkingTokenBudget":true}));
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Xhigh);
        options.thinking_budgets = Some(ThinkingBudgets {
            high: Some(8192),
            ..Default::default()
        });
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["thinking_token_budget"], json!(8192));
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Max);
        options.thinking_budgets = Some(ThinkingBudgets {
            high: Some(8192),
            ..Default::default()
        });
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["thinking_token_budget"], json!(8192));

        // Default high budget minus the answer room against the model cap.
        let model = vllm_model(json!({"thinkingFormat":"zai","supportsThinkingTokenBudget":true}));
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["thinking_token_budget"], json!(16384 - 1024));

        // Caller maxTokens caps the ceiling.
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        options.thinking_budgets = Some(ThinkingBudgets {
            high: Some(8192),
            ..Default::default()
        });
        options.stream.max_tokens = Some(4096);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["thinking_token_budget"], json!(4096 - 1024));
    }

    #[test]
    fn thinking_token_budget_field_overrides_the_alias() {
        for field in ["thinking_budget", "thinking_budget_tokens"] {
            let model =
                vllm_model(json!({"thinkingFormat":"qwen","thinkingTokenBudgetField": field}));
            let mut options = opts();
            options.reasoning = Some(ThinkingLevel::Medium);
            options.thinking_budgets = Some(ThinkingBudgets {
                medium: Some(4096),
                ..Default::default()
            });
            let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
            assert_eq!(body[field], json!(4096), "{field}");
            assert!(body
                .as_object()
                .unwrap()
                .get("thinking_token_budget")
                .is_none());
        }

        // Explicit field wins over the boolean alias.
        let model = vllm_model(json!({
            "thinkingFormat":"zai",
            "supportsThinkingTokenBudget": true,
            "thinkingTokenBudgetField": "thinking_budget"
        }));
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Medium);
        options.thinking_budgets = Some(ThinkingBudgets {
            medium: Some(4096),
            ..Default::default()
        });
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["thinking_budget"], json!(4096));
        assert!(body
            .as_object()
            .unwrap()
            .get("thinking_token_budget")
            .is_none());
    }

    #[test]
    fn chat_template_kwargs_receive_the_clamped_budget() {
        let model = vllm_model(json!({
            "thinkingFormat": "chat-template",
            "chatTemplateKwargs": {
                "enable_thinking": {"$var": "thinking.enabled"},
                "thinking_budget": {"$var": "thinking.budget"}
            }
        }));
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": true, "thinking_budget": 16384 - 1024})
        );
        assert!(body
            .as_object()
            .unwrap()
            .get("thinking_token_budget")
            .is_none());

        // Thinking off: the budget var is omitted, enable_thinking false.
        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": false})
        );
    }

    // ---- 11. thinking format variants ----

    #[test]
    fn zai_thinking_format_maps_levels_through_thinking_level_map() {
        let mut model = make_model(
            "zai",
            "https://api.z.ai/api/paas/v4",
            "glm-5.2",
            true,
            json!({"supportsReasoningEffort": true}),
        );
        let mut map = BTreeMap::new();
        map.insert("off".to_string(), Some("none".to_string()));
        map.insert("minimal".to_string(), None);
        map.insert("low".to_string(), None);
        map.insert("medium".to_string(), None);
        map.insert("high".to_string(), Some("high".to_string()));
        map.insert("xhigh".to_string(), None);
        map.insert("max".to_string(), Some("max".to_string()));
        model.thinking_level_map = Some(map);

        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Low);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(
            body["thinking"],
            json!({"type":"enabled","clear_thinking":false})
        );
        assert_eq!(body["reasoning_effort"], json!("high"));

        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Max);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["reasoning_effort"], json!("max"));

        // Off: disabled, no reasoning_effort.
        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert_eq!(body["thinking"], json!({"type":"disabled"}));
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());
    }

    #[test]
    fn openrouter_reasoning_object_replaces_reasoning_effort() {
        let model = make_model(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "deepseek/deepseek-r1",
            true,
            json!({}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["reasoning"], json!({"effort": "high"}));
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_thinking_format_toggles_thinking() {
        let model = make_model(
            "deepseek",
            "https://api.deepseek.com/v1",
            "deepseek-flash",
            true,
            json!({}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["thinking"], json!({"type": "enabled"}));
        assert_eq!(body["reasoning_effort"], json!("high"));

        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());
    }

    #[test]
    fn together_reasoning_enabled_flag() {
        // Detection disables reasoning_effort for Together; the compat flag
        // re-enables it (the path the must-cover "together -> reasoning +
        // effort" describes).
        let model = make_model(
            "together",
            "https://api.together.ai/v1",
            "m",
            true,
            json!({"supportsReasoningEffort": true}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["reasoning"], json!({"enabled": true}));
        assert_eq!(body["reasoning_effort"], json!("high"));

        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert_eq!(body["reasoning"], json!({"enabled": false}));
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());
    }

    #[test]
    fn baseten_chat_template_args_and_effort() {
        let model = make_model(
            "baseten",
            "https://api.baseten.co/v1",
            "m",
            true,
            json!({"thinkingFormat":"baseten","chatTemplateArgs":{"thinking":{"$var":"thinking.budget"}}}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        // make_model caps maxTokens at 4096, so the budget shares that ceiling.
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["chat_template_args"], json!({"thinking": 4096 - 1024}));
        assert_eq!(body["reasoning_effort"], json!("high"));

        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert!(body
            .as_object()
            .unwrap()
            .get("chat_template_args")
            .is_none());
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());
    }

    #[test]
    fn qwen_and_qwen_chat_template_flags() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "q",
            true,
            json!({"thinkingFormat":"qwen"}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["enable_thinking"], json!(true));
        assert_eq!(body["reasoning_effort"], json!("high"));
        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert_eq!(body["enable_thinking"], json!(false));

        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "Qwen/Qwen3-Coder",
            true,
            json!({"thinkingFormat":"qwen-chat-template","supportsReasoningEffort": false}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": true, "preserve_thinking": true})
        );
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());
        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": false, "preserve_thinking": true})
        );
    }

    #[test]
    fn chat_template_effort_kwarg_with_level_map() {
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "unsloth/gpt-oss-120b-GGUF",
            true,
            json!({
                "thinkingFormat":"chat-template",
                "supportsReasoningEffort": false,
                "chatTemplateKwargs": {
                    "preserve_thinking": true,
                    "reasoning_effort": {"$var":"thinking.effort","omitWhenOff": true}
                }
            }),
        );
        let mut map = BTreeMap::new();
        map.insert("xhigh".to_string(), Some("max".to_string()));
        model.thinking_level_map = Some(map);
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Xhigh);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"preserve_thinking": true, "reasoning_effort": "max"})
        );
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());

        // Off with omitWhenOff: only the static kwarg survives.
        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"preserve_thinking": true})
        );
    }

    #[test]
    fn string_thinking_format() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "m",
            true,
            json!({"thinkingFormat":"string-thinking"}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["thinking"], json!("high"));

        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert_eq!(body["thinking"], json!("none"));

        // map.off === null: no disabled thinking field at all.
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "m",
            true,
            json!({"thinkingFormat":"string-thinking"}),
        );
        let mut map = BTreeMap::new();
        map.insert("off".to_string(), None);
        model.thinking_level_map = Some(map);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert!(body.as_object().unwrap().get("thinking").is_none());
    }

    #[test]
    fn ant_ling_effort_only_when_mapped() {
        let mut model = make_model(
            "ant-ling",
            "https://api.ant-ling.com/v1",
            "Ring-2.6-1T",
            true,
            json!({}),
        );
        let mut map = BTreeMap::new();
        map.insert("high".to_string(), Some("high".to_string()));
        model.thinking_level_map = Some(map);
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["reasoning"], json!({"effort": "high"}));
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());

        // Unmapped level: no reasoning field.
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Medium);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert!(body.as_object().unwrap().get("reasoning").is_none());

        // Non-reasoning model: no reasoning field.
        let non_reasoning = make_model(
            "ant-ling",
            "https://api.ant-ling.com/v1",
            "Ling-2.6-flash",
            false,
            json!({}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&non_reasoning, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert!(body.as_object().unwrap().get("reasoning").is_none());
    }

    #[test]
    fn openai_reasoning_effort_with_level_map() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-5.5",
            true,
            json!({}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::High);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["reasoning_effort"], json!("high"));

        // Groq Qwen-style mapping: medium -> "default".
        let mut groq = make_model(
            "groq",
            "https://api.groq.com/openai/v1",
            "qwen/qwen3.6-27b",
            true,
            json!({}),
        );
        let mut map = BTreeMap::new();
        map.insert("medium".to_string(), Some("default".to_string()));
        groq.thinking_level_map = Some(map);
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Medium);
        let body = build(&groq, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["reasoning_effort"], json!("default"));

        // Without a mapping the level passes through.
        let groq = make_model(
            "groq",
            "https://api.groq.com/openai/v1",
            "openai/gpt-oss-20b",
            true,
            json!({}),
        );
        let mut options = opts();
        options.reasoning = Some(ThinkingLevel::Medium);
        let body = build(&groq, &ctx_of(vec![user("Hi")]), &options).unwrap();
        assert_eq!(body["reasoning_effort"], json!("medium"));

        // supportsReasoningEffort false: no field even with reasoning on.
        let mut model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "m",
            true,
            json!({"supportsReasoningEffort": false}),
        );
        let mut map = BTreeMap::new();
        map.insert("off".to_string(), Some("low".to_string()));
        model.thinking_level_map = Some(map);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert!(body.as_object().unwrap().get("reasoning_effort").is_none());

        // supportsReasoningEffort true + off + map.off string: effort from map.
        let mut model = make_model("openai", "https://api.openai.com/v1", "m", true, json!({}));
        let mut map = BTreeMap::new();
        map.insert("off".to_string(), Some("low".to_string()));
        model.thinking_level_map = Some(map);
        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert_eq!(body["reasoning_effort"], json!("low"));
    }

    // ---- 12. prompt cache + session affinity ----

    #[test]
    fn prompt_cache_key_and_retention() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let mut options = opts();
        options.stream.session_id = Some("session-123".to_string());
        let body = build(&model, &prompt_ctx("sys", vec![user("hi")], None), &options).unwrap();
        assert_eq!(body["prompt_cache_key"], json!("session-123"));
        assert!(body
            .as_object()
            .unwrap()
            .get("prompt_cache_retention")
            .is_none());

        // Long retention adds the 24h marker.
        let mut options = opts();
        options.stream.session_id = Some("session-456".to_string());
        options.stream.cache_retention = Some(CacheRetention::Long);
        let body = build(&model, &prompt_ctx("sys", vec![user("hi")], None), &options).unwrap();
        assert_eq!(body["prompt_cache_key"], json!("session-456"));
        assert_eq!(body["prompt_cache_retention"], json!("24h"));

        // 64-char clamp.
        let mut options = opts();
        options.stream.session_id = Some("x".repeat(67));
        let body = build(&model, &prompt_ctx("sys", vec![user("hi")], None), &options).unwrap();
        assert_eq!(body["prompt_cache_key"], json!("x".repeat(64)));

        // Retention none omits both fields.
        let mut options = opts();
        options.stream.session_id = Some("session-789".to_string());
        options.stream.cache_retention = Some(CacheRetention::None);
        let body = build(&model, &prompt_ctx("sys", vec![user("hi")], None), &options).unwrap();
        assert!(body.as_object().unwrap().get("prompt_cache_key").is_none());
        assert!(body
            .as_object()
            .unwrap()
            .get("prompt_cache_retention")
            .is_none());

        // Non-OpenAI URL without long-retention support: both absent even for long.
        let model = make_model(
            "proxy",
            "https://proxy.example.com/v1",
            "m",
            false,
            json!({"supportsLongCacheRetention": false}),
        );
        let mut options = opts();
        options.stream.session_id = Some("session-proxy".to_string());
        options.stream.cache_retention = Some(CacheRetention::Long);
        let body = build(&model, &prompt_ctx("sys", vec![user("hi")], None), &options).unwrap();
        assert!(body.as_object().unwrap().get("prompt_cache_key").is_none());
        assert!(body
            .as_object()
            .unwrap()
            .get("prompt_cache_retention")
            .is_none());

        // PI_CACHE_RETENTION env override.
        let mut options = opts();
        let mut env = ProviderEnv::default();
        env.insert("PI_CACHE_RETENTION".to_string(), "long".to_string());
        options.stream.env = Some(env);
        options.stream.session_id = Some("session-env".to_string());
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let body = build(&model, &prompt_ctx("sys", vec![user("hi")], None), &options).unwrap();
        assert_eq!(body["prompt_cache_key"], json!("session-env"));
        assert_eq!(body["prompt_cache_retention"], json!("24h"));
    }

    #[test]
    fn session_affinity_header_formats() {
        let base = "https://proxy.example.com/v1";

        // openai format: all three headers.
        let model = make_model(
            "p",
            base,
            "m",
            false,
            json!({"sendSessionAffinityHeaders": true}),
        );
        let mut options = opts();
        options.stream.session_id = Some("session-affinity".to_string());
        let assembly = build_request(
            &model,
            &cfg(base),
            &prompt_ctx("sys", vec![user("hi")], None),
            &options,
            &resolved(&model),
        )
        .unwrap();
        let get = |name: &str| {
            assembly
                .headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("session_id").as_deref(), Some("session-affinity"));
        assert_eq!(
            get("x-client-request-id").as_deref(),
            Some("session-affinity")
        );
        assert_eq!(
            get("x-session-affinity").as_deref(),
            Some("session-affinity")
        );
        assert!(assembly
            .headers
            .iter()
            .any(|(k, v)| k == "User-Agent" && v == &pi_user_agent()));

        // openai-nosession: pair without the session_id header.
        let model = make_model(
            "p",
            "https://api.openai.com/v1",
            "m",
            false,
            json!({"sendSessionAffinityHeaders": true, "sessionAffinityFormat": "openai-nosession"}),
        );
        let assembly = build_request(
            &model,
            &cfg(&model.base_url),
            &prompt_ctx("sys", vec![user("hi")], None),
            &options,
            &resolved(&model),
        )
        .unwrap();
        let get = |name: &str| {
            assembly
                .headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("session_id"), None);
        assert_eq!(
            get("x-client-request-id").as_deref(),
            Some("session-affinity")
        );
        assert_eq!(
            get("x-session-affinity").as_deref(),
            Some("session-affinity")
        );
        assert_eq!(get("x-session-id"), None);
        // The body still carries the cache key for the OpenAI URL.
        assert_eq!(assembly.body["prompt_cache_key"], json!("session-affinity"));

        // openrouter format: only x-session-id.
        let model = make_model(
            "p",
            base,
            "m",
            false,
            json!({"sendSessionAffinityHeaders": true, "sessionAffinityFormat": "openrouter"}),
        );
        let assembly = build_request(
            &model,
            &cfg(base),
            &prompt_ctx("sys", vec![user("hi")], None),
            &options,
            &resolved(&model),
        )
        .unwrap();
        let get = |name: &str| {
            assembly
                .headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("x-session-id").as_deref(), Some("session-affinity"));
        assert_eq!(get("session_id"), None);
        assert_eq!(get("x-client-request-id"), None);
        assert_eq!(get("x-session-affinity"), None);

        // cacheRetention none: no affinity headers at all.
        let model = make_model(
            "p",
            base,
            "m",
            false,
            json!({"sendSessionAffinityHeaders": true}),
        );
        let mut options = opts();
        options.stream.session_id = Some("session-affinity".to_string());
        options.stream.cache_retention = Some(CacheRetention::None);
        let assembly = build_request(
            &model,
            &cfg(base),
            &prompt_ctx("sys", vec![user("hi")], None),
            &options,
            &resolved(&model),
        )
        .unwrap();
        assert!(assembly
            .headers
            .iter()
            .all(|(k, _)| k != "x-session-affinity"
                && k != "session_id"
                && k != "x-client-request-id"));

        // Explicit headers override generated ones.
        let model = make_model(
            "p",
            base,
            "m",
            false,
            json!({"sendSessionAffinityHeaders": true}),
        );
        let mut options = opts();
        options.stream.session_id = Some("session-affinity".to_string());
        let mut headers = BTreeMap::new();
        headers.insert(
            "session_id".to_string(),
            Some("override-session".to_string()),
        );
        headers.insert(
            "x-client-request-id".to_string(),
            Some("override-request".to_string()),
        );
        headers.insert(
            "x-session-affinity".to_string(),
            Some("override-affinity".to_string()),
        );
        options.stream.headers = Some(headers);
        let assembly = build_request(
            &model,
            &cfg(base),
            &prompt_ctx("sys", vec![user("hi")], None),
            &options,
            &resolved(&model),
        )
        .unwrap();
        let get = |name: &str| {
            assembly
                .headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("session_id").as_deref(), Some("override-session"));
        assert_eq!(
            get("x-client-request-id").as_deref(),
            Some("override-request")
        );
        assert_eq!(
            get("x-session-affinity").as_deref(),
            Some("override-affinity")
        );

        // Disabled: no affinity data.
        let model = make_model(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "m",
            false,
            json!({"sendSessionAffinityHeaders": false}),
        );
        let mut options = opts();
        options.stream.session_id = Some("session-openrouter".to_string());
        let assembly = build_request(
            &model,
            &cfg(&model.base_url),
            &prompt_ctx("sys", vec![user("hi")], None),
            &options,
            &resolved(&model),
        )
        .unwrap();
        assert!(assembly.headers.iter().all(|(k, _)| k != "x-session-id"));
    }

    #[test]
    fn model_headers_merge_and_options_headers_override() {
        let mut model = make_model("p", "https://proxy.example.com/v1", "m", false, json!({}));
        let mut headers = BTreeMap::new();
        headers.insert("x-model".to_string(), Some("from-model".to_string()));
        model.headers = Some(headers);
        let mut options = opts();
        let mut option_headers = BTreeMap::new();
        option_headers.insert("x-model".to_string(), Some("from-options".to_string()));
        option_headers.insert("x-drop".to_string(), None);
        options.stream.headers = Some(option_headers);
        let assembly = build_request(
            &model,
            &cfg(&model.base_url),
            &prompt_ctx("sys", vec![user("hi")], None),
            &options,
            &resolved(&model),
        )
        .unwrap();
        let get = |name: &str| {
            assembly
                .headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("x-model").as_deref(), Some("from-options"));
        assert_eq!(get("x-drop"), None);
    }

    /// A model-level `None` header value (upstream `null`) suppresses the
    /// default header of the same name, matching the options-level merge
    /// semantics and upstream `mergeHeaders`/`providerHeadersToRecord`.
    #[test]
    fn model_null_header_value_suppresses_default() {
        let mut model = make_model("p", "https://proxy.example.com/v1", "m", false, json!({}));
        let mut headers = BTreeMap::new();
        headers.insert("user-agent".to_string(), None);
        headers.insert("x-model".to_string(), Some("from-model".to_string()));
        model.headers = Some(headers);
        let assembly = build_request(
            &model,
            &cfg(&model.base_url),
            &prompt_ctx("sys", vec![user("hi")], None),
            &opts(),
            &resolved(&model),
        )
        .unwrap();
        let get = |name: &str| {
            assembly
                .headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        // The pi default User-Agent is suppressed; the plain model header stays.
        assert!(assembly.headers.iter().all(|(k, _)| k != "user-agent"));
        assert_eq!(get("x-model").as_deref(), Some("from-model"));
    }

    // ---- 13. anthropic cache-control markers ----

    fn openrouter_anthropic_model() -> Model {
        make_model(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "anthropic/claude-fable-5.1:batch",
            true,
            json!({}),
        )
    }

    #[test]
    fn anthropic_cache_markers_on_system_last_tool_and_last_message() {
        let model = openrouter_anthropic_model();
        let body = build(
            &model,
            &prompt_ctx(
                "System prompt",
                vec![user("Hello")],
                Some(vec![plain_tool("read")]),
            ),
            &opts(),
        )
        .unwrap();
        assert_eq!(
            body["messages"][0]["content"],
            json!([{"type":"text","text":"System prompt","cache_control":{"type":"ephemeral"}}])
        );
        assert_eq!(
            body["tools"][0]["cache_control"],
            json!({"type":"ephemeral"})
        );
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["role"], json!("user"));
        assert_eq!(
            last["content"],
            json!([{"type":"text","text":"Hello","cache_control":{"type":"ephemeral"}}])
        );
    }

    #[test]
    fn anthropic_cache_marker_moves_to_tool_result() {
        let model = openrouter_anthropic_model();
        let messages = vec![
            user("Read the file"),
            assistant(
                "openrouter",
                "openai-completions",
                "anthropic/claude-fable-5.1:batch",
                vec![tool_call("call_1", "read", json!({"path":"README.md"}))],
                StopReason::ToolUse,
            ),
            tool_result("call_1", "read", vec![text_block("file contents")]),
        ];
        let body = build(
            &model,
            &prompt_ctx("System prompt", messages, Some(vec![plain_tool("read")])),
            &opts(),
        )
        .unwrap();
        let user_message = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == json!("user"))
            .unwrap();
        assert_eq!(user_message["content"], json!("Read the file"));
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["role"], json!("tool"));
        assert_eq!(
            last["content"],
            json!([{"type":"text","text":"file contents","cache_control":{"type":"ephemeral"}}])
        );
        assert_eq!(
            body["tools"][0]["cache_control"],
            json!({"type":"ephemeral"})
        );
    }

    #[test]
    fn anthropic_cache_markers_omitted_when_retention_none() {
        let model = openrouter_anthropic_model();
        let mut options = opts();
        options.stream.cache_retention = Some(CacheRetention::None);
        let body = build(
            &model,
            &prompt_ctx(
                "System prompt",
                vec![user("Hello")],
                Some(vec![plain_tool("read")]),
            ),
            &options,
        )
        .unwrap();
        assert_eq!(body["messages"][0]["content"], json!("System prompt"));
        assert!(body["tools"][0]
            .as_object()
            .unwrap()
            .get("cache_control")
            .is_none());
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["content"], json!("Hello"));
    }

    #[test]
    fn anthropic_cache_markers_carry_ttl_for_long_retention() {
        let model = openrouter_anthropic_model();
        let mut options = opts();
        options.stream.cache_retention = Some(CacheRetention::Long);
        let body = build(
            &model,
            &prompt_ctx(
                "System prompt",
                vec![user("Hello")],
                Some(vec![plain_tool("read")]),
            ),
            &options,
        )
        .unwrap();
        let ttl = json!({"type":"ephemeral","ttl":"1h"});
        assert_eq!(body["messages"][0]["content"][0]["cache_control"], ttl);
        assert_eq!(body["tools"][0]["cache_control"], ttl);
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["content"][0]["cache_control"], ttl);
    }

    /// The backward scan's empty-content case (upstream
    /// `addCacheControlToTextContent`: `content.length === 0` returns false):
    /// a wire message whose `content` is the empty string is left untouched
    /// and the scan keeps moving to an earlier message with text.
    #[test]
    fn anthropic_cache_marker_scan_skips_empty_string_content() {
        let marker = json!({"type":"ephemeral"});
        let mut messages = vec![
            json!({"role":"user","content":"earlier"}),
            json!({"role":"assistant","content":""}),
        ];
        add_cache_control_to_last_conversation_message(&mut messages, &marker);
        assert_eq!(
            messages[0]["content"],
            json!([{"type":"text","text":"earlier","cache_control":{"type":"ephemeral"}}])
        );
        assert_eq!(messages[1]["content"], json!(""));

        // Only empty-content messages: nothing is marked.
        let mut messages = vec![json!({"role":"assistant","content":""})];
        add_cache_control_to_last_conversation_message(&mut messages, &marker);
        assert_eq!(messages[0]["content"], json!(""));
    }

    // ---- 14. routing fields ----

    #[test]
    fn openrouter_routing_becomes_the_provider_field() {
        let model = make_model(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "m",
            false,
            json!({"openRouterRouting": {"only": ["deepseek"]}}),
        );
        let body = build(&model, &ctx_of(vec![user("hi")]), &opts()).unwrap();
        assert_eq!(body["provider"], json!({"only": ["deepseek"]}));

        // Without compat routing there is no provider field.
        let model = make_model("openai", "https://api.openai.com/v1", "m", false, json!({}));
        let body = build(&model, &ctx_of(vec![user("hi")]), &opts()).unwrap();
        assert!(body.as_object().unwrap().get("provider").is_none());
    }

    #[test]
    fn vercel_gateway_routing_becomes_provider_options() {
        let model = make_model(
            "vercel",
            "https://vercel.com/api/v1",
            "m",
            false,
            json!({"vercelGatewayRouting": {"only": ["bedrock"], "order": ["anthropic", "openai"]}}),
        );
        let body = build(&model, &ctx_of(vec![user("hi")]), &opts()).unwrap();
        assert_eq!(
            body["providerOptions"],
            json!({"gateway": {"only": ["bedrock"], "order": ["anthropic", "openai"]}})
        );
    }

    #[test]
    fn vllm_priority_field() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "m",
            false,
            json!({"vllmPriority": 10}),
        );
        let body = build(&model, &ctx_of(vec![user("hi")]), &opts()).unwrap();
        assert_eq!(body["priority"], json!(10));

        let model = make_model("openai", "https://api.openai.com/v1", "m", false, json!({}));
        let body = build(&model, &ctx_of(vec![user("hi")]), &opts()).unwrap();
        assert!(body.as_object().unwrap().get("priority").is_none());
    }

    // ---- 15. tool choice ----

    #[test]
    fn tool_choice_maps_auto_and_none() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        let mut options = opts();
        options.tool_choice = Some(ToolChoice::Auto);
        let body = build(
            &model,
            &prompt_ctx(
                "sys",
                vec![user("Call ping")],
                Some(vec![plain_tool("ping")]),
            ),
            &options,
        )
        .unwrap();
        assert_eq!(body["tool_choice"], json!("auto"));
        assert!(!body["tools"].as_array().unwrap().is_empty());

        // toolChoice none without tools: no tools key at all.
        let mut options = opts();
        options.tool_choice = Some(ToolChoice::None);
        let body = build(&model, &ctx_of(vec![user("Summarize")]), &options).unwrap();
        assert_eq!(body["tool_choice"], json!("none"));
        assert!(!body.as_object().unwrap().contains_key("tools"));
    }

    #[test]
    fn zai_tool_stream_follows_tools_presence() {
        let model = make_model(
            "zai",
            "https://api.z.ai/api/paas/v4",
            "glm-5.2",
            true,
            json!({"zaiToolStream": true}),
        );
        let body = build(
            &model,
            &prompt_ctx(
                "sys",
                vec![user("Call ping")],
                Some(vec![plain_tool("ping")]),
            ),
            &opts(),
        )
        .unwrap();
        assert_eq!(body["tool_stream"], json!(true));

        let body = build(&model, &ctx_of(vec![user("Hi")]), &opts()).unwrap();
        assert!(body.as_object().unwrap().get("tool_stream").is_none());
    }

    // ---- 16. unit: hash + id normalization + cache-key clamp ----

    #[test]
    fn short_hash_matches_upstream() {
        // Reference values computed with the upstream utils/hash.ts
        // implementation under node.
        assert_eq!(short_hash("abc123|ws/item|+3d=="), "1pnp6duik20ve");
        assert_eq!(short_hash("x"), "evpdwv6p0anp");
        assert_eq!(short_hash(""), "k4n83c7h0j2b");
    }

    #[test]
    fn normalize_tool_call_id_cases() {
        let model = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        assert_eq!(
            super::normalize_tool_call_id(&model, "abc123|ws/item|+3d=="),
            "abc123_ws_item__3d__"
        );
        assert_eq!(super::normalize_tool_call_id(&model, "abc123|"), "abc123");
        let long = format!("{}|{}", "a".repeat(45), "b".repeat(45));
        assert_eq!(
            super::normalize_tool_call_id(&model, &long),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa_1l9wu4r1"
        );
        // Non-pipe ids pass through except for the openai 40-char truncation.
        let non_openai = make_model(
            "proxy",
            "https://proxy.example.com/v1",
            "m",
            false,
            json!({}),
        );
        assert_eq!(
            super::normalize_tool_call_id(&non_openai, "short_id"),
            "short_id"
        );
        let long_plain = "0123456789012345678901234567890123456789ABC";
        assert_eq!(
            super::normalize_tool_call_id(&non_openai, long_plain),
            long_plain
        );
        let openai = make_model(
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            false,
            json!({}),
        );
        assert_eq!(
            super::normalize_tool_call_id(&openai, long_plain),
            &long_plain[..40]
        );
    }
}
