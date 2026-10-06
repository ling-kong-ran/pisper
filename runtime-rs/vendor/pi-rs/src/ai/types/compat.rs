//! Per-API compatibility settings from upstream `packages/ai/src/types.ts`
//! (`OpenAICompletionsCompat` 667-744, `OpenAIResponsesCompat` 747-768,
//! `AnthropicMessagesCompat` 771-833, `BedrockCompat` 836-839,
//! `MistralConversationsCompat` 842-845, `OpenRouterRouting` 853-920,
//! `VercelGatewayRouting` 927-932, plus `AnthropicAllowedFallbackModel`
//! 311-315). These objects ride on `Model.compat` in model-catalog JSON, so the
//! wire format must match the TypeScript interfaces byte-for-byte: struct field
//! names are the upstream camelCase names (`supportsStore`, `maxTokensField`,
//! `supportsOpenAIGrammarTools`, `openRouterRouting`), `OpenRouterRouting`
//! keeps upstream's snake_case request-body names (`allow_fallbacks`,
//! `data_collection`), and enum fields serialize the upstream literal union
//! values (`thinkingFormat: "zai"`, `maxTokensField: "max_tokens"`, ...). All
//! settings are optional: missing JSON keys deserialize to `None` and `None`
//! fields are omitted from JSON (like upstream `undefined`), so provider code
//! applies its own documented default when a field is unset.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::primitives::{
    ChatTemplateKwargValue, ModelCost, SessionAffinityFormat, ThinkingTokenBudgetField,
};

/// Upstream `OpenAICompletionsCompat.maxTokensField` (types.ts:679): which
/// request field caps completion tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensField {
    MaxCompletionTokens,
    MaxTokens,
}

/// Upstream `OpenAICompletionsCompat.thinkingFormat` (types.ts:689-700): the
/// reasoning/thinking parameter convention for OpenAI-compatible endpoints.
/// Wire values are the upstream literals exactly (`"zai"`, `"chat-template"`,
/// `"ant-ling"`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingFormat {
    Openai,
    Openrouter,
    Deepseek,
    Together,
    Baseten,
    Zai,
    Qwen,
    ChatTemplate,
    QwenChatTemplate,
    StringThinking,
    AntLing,
}

/// Upstream `OpenAICompletionsCompat.cacheControlFormat` (types.ts:730): the
/// prompt-caching convention. Upstream only defines `"anthropic"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheControlFormat {
    Anthropic,
}

/// Upstream `AnthropicMessagesCompat.sessionAffinityFormat` (types.ts:791):
/// upstream restricts this field to `"openrouter"` (unlike the completions
/// variant, which uses the three-valued `SessionAffinityFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicSessionAffinityFormat {
    Openrouter,
}

/// Upstream `OpenRouterRouting.data_collection` (types.ts:859): whether
/// providers that store/train on data may serve the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataCollection {
    Deny,
    Allow,
}

/// Upstream `AnthropicAllowedFallbackModel` (types.ts:311-315): a model
/// Anthropic accepts in server-side refusal `fallbacks`, with local pricing for
/// returned fallback responses. All fields are required upstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicAllowedFallbackModel {
    /// Upstream `ProviderId` (`KnownProvider | string`); open-ended like the
    /// provider ids checked against `KNOWN_PROVIDERS`.
    pub provider: String,
    pub model: String,
    pub cost: ModelCost,
}

/// Upstream `OpenAICompletionsCompat` (types.ts:667-744): compatibility
/// overrides for OpenAI-compatible completions APIs. Defaults are
/// URL-auto-detected upstream, so every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiCompletionsCompat {
    /// Provider supports the `store` request field (types.ts:669).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_store: Option<bool>,
    /// Provider supports the `developer` role vs `system` (types.ts:671).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_developer_role: Option<bool>,
    /// Provider supports `reasoning_effort` (types.ts:673).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_effort: Option<bool>,
    /// Provider supports `stream_options.include_usage` in streaming (types.ts:675).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_usage_in_streaming: Option<bool>,
    /// Streamed responses include `finish_reason` (types.ts:677).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_finish_reason: Option<bool>,
    /// Request field used for max tokens (types.ts:679).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens_field: Option<MaxTokensField>,
    /// Tool results require the `name` field (types.ts:681).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_tool_result_name: Option<bool>,
    /// A user message after tool results requires an assistant message between
    /// (types.ts:683).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_assistant_after_tool_result: Option<bool>,
    /// Thinking blocks must become `<thinking>`-delimited text (types.ts:685).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_thinking_as_text: Option<bool>,
    /// Replayed assistant messages need an empty `reasoning_content` field
    /// (types.ts:687).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_reasoning_content_on_assistant_messages: Option<bool>,
    /// Reasoning/thinking parameter convention (types.ts:689-700).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<ThinkingFormat>,
    /// `chat_template_kwargs` payload when `thinkingFormat` is `chat-template`
    /// or `qwen-chat-template` (types.ts:702).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<BTreeMap<String, ChatTemplateKwargValue>>,
    /// `chat_template_args` payload when `thinkingFormat` is `baseten`
    /// (types.ts:704).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_args: Option<BTreeMap<String, ChatTemplateKwargValue>>,
    /// OpenRouter routing preferences sent as the `provider` request field
    /// (types.ts:706).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_router_routing: Option<OpenRouterRouting>,
    /// Vercel AI Gateway routing preferences (types.ts:708).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vercel_gateway_routing: Option<VercelGatewayRouting>,
    /// z.ai top-level `tool_stream: true` for streaming tool deltas (types.ts:710).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zai_tool_stream: Option<bool>,
    /// Request field capping reasoning tokens from `thinkingBudgets`
    /// (types.ts:718).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_token_budget_field: Option<ThinkingTokenBudgetField>,
    /// Alias for `thinkingTokenBudgetField: "thinking_token_budget"` (types.ts:720).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_thinking_token_budget: Option<bool>,
    /// Provider supports OpenAI custom tools with Lark/regex grammars
    /// (types.ts:722). Upstream spells the wire name with capital "AI".
    #[serde(
        rename = "supportsOpenAIGrammarTools",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_openai_grammar_tools: Option<bool>,
    /// System/developer messages accepted mid-conversation (types.ts:724).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_mid_convo_system_messages: Option<bool>,
    /// Tools can be introduced by mid-conversation system messages; requires
    /// `supportsMidConvoSystemMessages` (types.ts:726).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_mid_convo_tool_additions: Option<bool>,
    /// Provider supports `strict` in tool definitions (types.ts:728).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
    /// Prompt-caching convention; upstream only defines `"anthropic"`
    /// (types.ts:730).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control_format: Option<CacheControlFormat>,
    /// Send session-affinity data from `options.sessionId` (types.ts:732).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_session_affinity_headers: Option<bool>,
    /// Session-affinity header format (types.ts:734).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_affinity_format: Option<SessionAffinityFormat>,
    /// Provider supports long prompt cache retention (types.ts:736).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    /// vLLM scheduler priority sent as the top-level `priority` request field
    /// (types.ts:743). `serde_json::Number` keeps integers integer on
    /// reserialization, matching upstream JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vllm_priority: Option<serde_json::Number>,
}

/// Upstream `OpenAIResponsesCompat` (types.ts:747-768): compatibility overrides
/// for OpenAI Responses APIs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiResponsesCompat {
    /// Provider supports the `developer` role vs `system` (types.ts:749).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_developer_role: Option<bool>,
    /// System/developer messages accepted mid-conversation (types.ts:751).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_mid_convo_system_messages: Option<bool>,
    /// Session-affinity header format (types.ts:753).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_affinity_format: Option<SessionAffinityFormat>,
    /// Provider supports long prompt cache retention (types.ts:755).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    /// Provider supports strict JSON-schema function tools (types.ts:757).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
    /// OpenAI custom tools with Lark/regex grammar formats are emitted
    /// (types.ts:759). Upstream spells the wire name with capital "AI".
    #[serde(
        rename = "supportsOpenAIGrammarTools",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_openai_grammar_tools: Option<bool>,
    /// Message-anchored `additional_tools` input items are supported (types.ts:761).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_additional_tools: Option<bool>,
    /// Client-executed tool search for transcript-anchored additions (types.ts:763).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_tool_search: Option<bool>,
    /// Model accepts `prompt_cache_options` (OpenAI GPT-5.6+ caching) (types.ts:765).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_explicit_prompt_cache_mode: Option<bool>,
    /// Provider accepts the `max_output_tokens` parameter (types.ts:767).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_max_output_tokens: Option<bool>,
}

/// Upstream `AnthropicMessagesCompat` (types.ts:771-833): compatibility
/// overrides for Anthropic Messages-compatible APIs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicMessagesCompat {
    /// Per-tool `eager_input_streaming` accepted; when false the legacy
    /// fine-grained-tool-streaming beta header is sent instead (types.ts:779).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_eager_tool_input_streaming: Option<bool>,
    /// Anthropic `cache_control.ttl: "1h"` supported (types.ts:781).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    /// Send `x-session-affinity` from `options.sessionId` when caching is
    /// enabled (types.ts:789).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_session_affinity_headers: Option<bool>,
    /// Session-affinity format; `"openrouter"` sends `x-session-id`
    /// (types.ts:791).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_affinity_format: Option<AnthropicSessionAffinityFormat>,
    /// Anthropic `cache_control` markers allowed on tool definitions (types.ts:799).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_cache_control_on_tools: Option<bool>,
    /// Anthropic `temperature` request field accepted (types.ts:805).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_temperature: Option<bool>,
    /// Force adaptive thinking (`thinking.type: "adaptive"` plus
    /// `output_config.effort`) regardless of model id (types.ts:815).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_adaptive_thinking: Option<bool>,
    /// Replay empty thinking signatures as `signature: ""` instead of
    /// converting thinking to text (types.ts:817).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_empty_signature: Option<bool>,
    /// Anthropic strict tool schemas supported (types.ts:819).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_strict_tools: Option<bool>,
    /// Transport supports effort-only system messages and thinking binding
    /// controls (types.ts:821).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_mid_convo_effort: Option<bool>,
    /// System-role messages accepted mid-conversation (types.ts:823).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_mid_convo_system_messages: Option<bool>,
    /// Mid-conversation `tool_addition`/`tool_removal` blocks accepted;
    /// requires `supportsMidConvoSystemMessages` (types.ts:825).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_mid_convo_tool_changes: Option<bool>,
    /// Models Anthropic accepts in server-side refusal `fallbacks`; absent or
    /// empty means callers must omit `fallbacks` (types.ts:832).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_fallback_models: Option<Vec<AnthropicAllowedFallbackModel>>,
}

/// Upstream `BedrockCompat` (types.ts:836-839): compatibility overrides for
/// Amazon Bedrock models.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BedrockCompat {
    /// Model supports Bedrock strict tool schemas (types.ts:838).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
}

/// Upstream `MistralConversationsCompat` (types.ts:842-845): compatibility
/// overrides for the Mistral chat API.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MistralConversationsCompat {
    /// System messages accepted after the conversation has started (types.ts:844).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_mid_convo_system_messages: Option<bool>,
}

/// Upstream `OpenRouterRouting.sort` (types.ts:873-880): a bare sorting
/// strategy string, or an object with a metric and an optional partition
/// (`string | null` — explicit null stays null on the wire, distinct from
/// absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpenRouterSort {
    Strategy(String),
    Object {
        #[serde(skip_serializing_if = "Option::is_none")]
        by: Option<String>,
        #[serde(
            default,
            deserialize_with = "double_option",
            skip_serializing_if = "Option::is_none"
        )]
        partition: Option<Option<String>>,
    },
}

/// Member of `OpenRouterRouting.max_price` (types.ts:884-892): a price as a
/// JSON number or string. `serde_json::Number` keeps integers integer on
/// reserialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpenRouterPrice {
    Number(serde_json::Number),
    String(String),
}

/// Percentile cutoff object of `preferred_min_throughput` /
/// `preferred_max_latency` (types.ts:898-905, 910-918).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterPercentiles {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50: Option<serde_json::Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p75: Option<serde_json::Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p90: Option<serde_json::Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p99: Option<serde_json::Number>,
}

/// Upstream `preferred_min_throughput` / `preferred_max_latency`
/// (types.ts:895-918): a bare number (applies to p50) or per-percentile
/// cutoffs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpenRouterNumberOrPercentiles {
    Number(serde_json::Number),
    Percentiles(OpenRouterPercentiles),
}

/// Upstream `OpenRouterRouting.max_price` (types.ts:882-893): maximum price
/// per million tokens (USD) per dimension.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterMaxPrice {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<OpenRouterPrice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion: Option<OpenRouterPrice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<OpenRouterPrice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<OpenRouterPrice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<OpenRouterPrice>,
}

/// Upstream `OpenRouterRouting` (types.ts:853-920): provider routing
/// preferences sent as the `provider` field of OpenRouter request bodies.
/// Field names are upstream's snake_case request-body names verbatim.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterRouting {
    /// Allow backup providers to serve requests (types.ts:855).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,
    /// Only providers supporting every request parameter (types.ts:857).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_parameters: Option<bool>,
    /// Data collection restriction (types.ts:859).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_collection: Option<DataCollection>,
    /// Restrict to Zero Data Retention endpoints (types.ts:861).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zdr: Option<bool>,
    /// Restrict to models allowing text distillation (types.ts:863).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enforce_distillable_text: Option<bool>,
    /// Provider names/slugs to try in sequence (types.ts:865).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
    /// Providers exclusively allowed for this request (types.ts:867).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    /// Providers to skip for this request (types.ts:869).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore: Option<Vec<String>>,
    /// Quantization levels to filter providers by (types.ts:871).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantizations: Option<Vec<String>>,
    /// Sorting strategy: string or `{ by, partition }` (types.ts:873-880).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<OpenRouterSort>,
    /// Maximum price per million tokens (USD) (types.ts:882-893).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_price: Option<OpenRouterMaxPrice>,
    /// Preferred minimum throughput in tokens/second (types.ts:895-906).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_min_throughput: Option<OpenRouterNumberOrPercentiles>,
    /// Preferred maximum latency in seconds (types.ts:907-919).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_max_latency: Option<OpenRouterNumberOrPercentiles>,
}

/// Upstream `VercelGatewayRouting` (types.ts:927-932): provider routing
/// preferences for the Vercel AI Gateway.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VercelGatewayRouting {
    /// Provider slugs exclusively used for this request (types.ts:929).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    /// Provider slugs to try in order (types.ts:931).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
}

/// Deserializes `T` and wraps the result in an outer `Some`, so `null` maps to
/// `Some(None)` (present-but-null) instead of `None` (absent). Required for
/// upstream `string | null` optional fields; `#[serde(default)]` covers the
/// absent case.
fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

#[cfg(test)]
mod tests {
    use super::super::primitives::ChatTemplateVariable;
    use super::*;

    const CATALOG_FIXTURE: &str = r#"{
  "completions": {
    "supportsStore": false,
    "supportsDeveloperRole": true,
    "supportsReasoningEffort": false,
    "supportsUsageInStreaming": false,
    "supportsFinishReason": false,
    "maxTokensField": "max_tokens",
    "requiresToolResultName": true,
    "requiresAssistantAfterToolResult": true,
    "requiresThinkingAsText": true,
    "requiresReasoningContentOnAssistantMessages": true,
    "thinkingFormat": "zai",
    "chatTemplateKwargs": {
      "enable_thinking": {"$var": "thinking.enabled", "omitWhenOff": true},
      "preserve_thinking": true
    },
    "chatTemplateArgs": {"thinking": {"$var": "thinking.budget"}},
    "openRouterRouting": {
      "allow_fallbacks": false,
      "require_parameters": true,
      "data_collection": "deny",
      "zdr": true,
      "enforce_distillable_text": false,
      "order": ["anthropic", "openai"],
      "only": ["deepseek"],
      "ignore": ["groq"],
      "quantizations": ["fp16", "fp8"],
      "sort": {"by": "price", "partition": null},
      "max_price": {"prompt": "0.5", "completion": 1.5, "image": "2", "audio": 0.25, "request": "0"},
      "preferred_min_throughput": {"p50": 40, "p99": 12},
      "preferred_max_latency": 2.5
    },
    "vercelGatewayRouting": {"only": ["bedrock"], "order": ["anthropic", "openai"]},
    "zaiToolStream": true,
    "thinkingTokenBudgetField": "thinking_budget_tokens",
    "supportsThinkingTokenBudget": true,
    "supportsOpenAIGrammarTools": true,
    "supportsMidConvoSystemMessages": true,
    "supportsMidConvoToolAdditions": true,
    "supportsStrictMode": false,
    "cacheControlFormat": "anthropic",
    "sendSessionAffinityHeaders": true,
    "sessionAffinityFormat": "openai-nosession",
    "supportsLongCacheRetention": false,
    "vllmPriority": -1
  },
  "responses": {
    "supportsDeveloperRole": false,
    "supportsMidConvoSystemMessages": true,
    "sessionAffinityFormat": "openrouter",
    "supportsLongCacheRetention": true,
    "supportsStrictMode": true,
    "supportsOpenAIGrammarTools": true,
    "supportsAdditionalTools": true,
    "supportsToolSearch": false,
    "supportsExplicitPromptCacheMode": true,
    "supportsMaxOutputTokens": false
  },
  "anthropic": {
    "supportsEagerToolInputStreaming": false,
    "supportsLongCacheRetention": false,
    "sendSessionAffinityHeaders": true,
    "sessionAffinityFormat": "openrouter",
    "supportsCacheControlOnTools": false,
    "supportsTemperature": false,
    "forceAdaptiveThinking": true,
    "allowEmptySignature": true,
    "supportsStrictTools": true,
    "supportsMidConvoEffort": true,
    "supportsMidConvoSystemMessages": true,
    "supportsMidConvoToolChanges": true,
    "allowedFallbackModels": [
      {
        "provider": "anthropic",
        "model": "claude-opus-4-6",
        "cost": {"input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 3.75}
      }
    ]
  },
  "bedrock": {"supportsStrictMode": true},
  "mistral": {"supportsMidConvoSystemMessages": true}
}"#;

    /// Same object in compact form (what `serde_json::to_string` emits); the
    /// byte-pinned round-trip target.
    const CATALOG_FIXTURE_COMPACT: &str = r#"{"completions":{"supportsStore":false,"supportsDeveloperRole":true,"supportsReasoningEffort":false,"supportsUsageInStreaming":false,"supportsFinishReason":false,"maxTokensField":"max_tokens","requiresToolResultName":true,"requiresAssistantAfterToolResult":true,"requiresThinkingAsText":true,"requiresReasoningContentOnAssistantMessages":true,"thinkingFormat":"zai","chatTemplateKwargs":{"enable_thinking":{"$var":"thinking.enabled","omitWhenOff":true},"preserve_thinking":true},"chatTemplateArgs":{"thinking":{"$var":"thinking.budget"}},"openRouterRouting":{"allow_fallbacks":false,"require_parameters":true,"data_collection":"deny","zdr":true,"enforce_distillable_text":false,"order":["anthropic","openai"],"only":["deepseek"],"ignore":["groq"],"quantizations":["fp16","fp8"],"sort":{"by":"price","partition":null},"max_price":{"prompt":"0.5","completion":1.5,"image":"2","audio":0.25,"request":"0"},"preferred_min_throughput":{"p50":40,"p99":12},"preferred_max_latency":2.5},"vercelGatewayRouting":{"only":["bedrock"],"order":["anthropic","openai"]},"zaiToolStream":true,"thinkingTokenBudgetField":"thinking_budget_tokens","supportsThinkingTokenBudget":true,"supportsOpenAIGrammarTools":true,"supportsMidConvoSystemMessages":true,"supportsMidConvoToolAdditions":true,"supportsStrictMode":false,"cacheControlFormat":"anthropic","sendSessionAffinityHeaders":true,"sessionAffinityFormat":"openai-nosession","supportsLongCacheRetention":false,"vllmPriority":-1},"responses":{"supportsDeveloperRole":false,"supportsMidConvoSystemMessages":true,"sessionAffinityFormat":"openrouter","supportsLongCacheRetention":true,"supportsStrictMode":true,"supportsOpenAIGrammarTools":true,"supportsAdditionalTools":true,"supportsToolSearch":false,"supportsExplicitPromptCacheMode":true,"supportsMaxOutputTokens":false},"anthropic":{"supportsEagerToolInputStreaming":false,"supportsLongCacheRetention":false,"sendSessionAffinityHeaders":true,"sessionAffinityFormat":"openrouter","supportsCacheControlOnTools":false,"supportsTemperature":false,"forceAdaptiveThinking":true,"allowEmptySignature":true,"supportsStrictTools":true,"supportsMidConvoEffort":true,"supportsMidConvoSystemMessages":true,"supportsMidConvoToolChanges":true,"allowedFallbackModels":[{"provider":"anthropic","model":"claude-opus-4-6","cost":{"input":3,"output":15,"cacheRead":0.3,"cacheWrite":3.75}}]},"bedrock":{"supportsStrictMode":true},"mistral":{"supportsMidConvoSystemMessages":true}}"#;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CatalogFixture {
        completions: OpenAiCompletionsCompat,
        responses: OpenAiResponsesCompat,
        anthropic: AnthropicMessagesCompat,
        bedrock: BedrockCompat,
        mistral: MistralConversationsCompat,
    }

    #[test]
    fn thinking_format_wire_values_match_upstream() {
        let cases = [
            (ThinkingFormat::Openai, "\"openai\""),
            (ThinkingFormat::Openrouter, "\"openrouter\""),
            (ThinkingFormat::Deepseek, "\"deepseek\""),
            (ThinkingFormat::Together, "\"together\""),
            (ThinkingFormat::Baseten, "\"baseten\""),
            (ThinkingFormat::Zai, "\"zai\""),
            (ThinkingFormat::Qwen, "\"qwen\""),
            (ThinkingFormat::ChatTemplate, "\"chat-template\""),
            (ThinkingFormat::QwenChatTemplate, "\"qwen-chat-template\""),
            (ThinkingFormat::StringThinking, "\"string-thinking\""),
            (ThinkingFormat::AntLing, "\"ant-ling\""),
        ];
        assert_eq!(cases.len(), 11);
        for (value, wire) in cases {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(serde_json::from_str::<ThinkingFormat>(wire).unwrap(), value);
        }
        assert!(serde_json::from_str::<ThinkingFormat>("\"unknown\"").is_err());
    }

    #[test]
    fn max_tokens_field_wire_values_match_upstream() {
        let cases = [
            (
                MaxTokensField::MaxCompletionTokens,
                "\"max_completion_tokens\"",
            ),
            (MaxTokensField::MaxTokens, "\"max_tokens\""),
        ];
        assert_eq!(cases.len(), 2);
        for (value, wire) in cases {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(serde_json::from_str::<MaxTokensField>(wire).unwrap(), value);
        }
        assert!(serde_json::from_str::<MaxTokensField>("\"max_output_tokens\"").is_err());
    }

    #[test]
    fn single_value_and_data_collection_enums_match_upstream() {
        assert_eq!(
            serde_json::to_string(&CacheControlFormat::Anthropic).unwrap(),
            "\"anthropic\""
        );
        assert_eq!(
            serde_json::from_str::<CacheControlFormat>("\"anthropic\"").unwrap(),
            CacheControlFormat::Anthropic
        );
        assert!(serde_json::from_str::<CacheControlFormat>("\"openai\"").is_err());

        assert_eq!(
            serde_json::to_string(&AnthropicSessionAffinityFormat::Openrouter).unwrap(),
            "\"openrouter\""
        );
        assert_eq!(
            serde_json::from_str::<AnthropicSessionAffinityFormat>("\"openrouter\"").unwrap(),
            AnthropicSessionAffinityFormat::Openrouter
        );
        assert!(serde_json::from_str::<AnthropicSessionAffinityFormat>("\"openai\"").is_err());

        assert_eq!(
            serde_json::to_string(&DataCollection::Deny).unwrap(),
            "\"deny\""
        );
        assert_eq!(
            serde_json::to_string(&DataCollection::Allow).unwrap(),
            "\"allow\""
        );
        assert_eq!(
            serde_json::from_str::<DataCollection>("\"deny\"").unwrap(),
            DataCollection::Deny
        );
        assert_eq!(
            serde_json::from_str::<DataCollection>("\"allow\"").unwrap(),
            DataCollection::Allow
        );
    }

    #[test]
    fn catalog_compat_fields_deserialize_and_round_trip() {
        let parsed: CatalogFixture = serde_json::from_str(CATALOG_FIXTURE).unwrap();

        let c = &parsed.completions;
        assert_eq!(c.supports_store, Some(false));
        assert_eq!(c.supports_developer_role, Some(true));
        assert_eq!(c.supports_reasoning_effort, Some(false));
        assert_eq!(c.supports_usage_in_streaming, Some(false));
        assert_eq!(c.supports_finish_reason, Some(false));
        assert_eq!(c.max_tokens_field, Some(MaxTokensField::MaxTokens));
        assert_eq!(c.requires_tool_result_name, Some(true));
        assert_eq!(c.requires_assistant_after_tool_result, Some(true));
        assert_eq!(c.requires_thinking_as_text, Some(true));
        assert_eq!(
            c.requires_reasoning_content_on_assistant_messages,
            Some(true)
        );
        assert_eq!(c.thinking_format, Some(ThinkingFormat::Zai));
        let kwargs = c.chat_template_kwargs.as_ref().unwrap();
        assert_eq!(
            kwargs.get("enable_thinking"),
            Some(&ChatTemplateKwargValue::Variable {
                var: ChatTemplateVariable::ThinkingEnabled,
                omit_when_off: Some(true),
            })
        );
        assert_eq!(
            kwargs.get("preserve_thinking"),
            Some(&ChatTemplateKwargValue::Bool(true))
        );
        let args = c.chat_template_args.as_ref().unwrap();
        assert_eq!(
            args.get("thinking"),
            Some(&ChatTemplateKwargValue::Variable {
                var: ChatTemplateVariable::ThinkingBudget,
                omit_when_off: None,
            })
        );
        let routing = c.open_router_routing.as_ref().unwrap();
        assert_eq!(routing.allow_fallbacks, Some(false));
        assert_eq!(routing.require_parameters, Some(true));
        assert_eq!(routing.data_collection, Some(DataCollection::Deny));
        assert_eq!(routing.zdr, Some(true));
        assert_eq!(routing.enforce_distillable_text, Some(false));
        let order = routing.order.as_ref().unwrap();
        assert_eq!(order.as_slice(), ["anthropic", "openai"]);
        assert_eq!(
            routing.only.as_deref(),
            Some(["deepseek".to_string()].as_slice())
        );
        assert_eq!(
            routing.ignore.as_deref(),
            Some(["groq".to_string()].as_slice())
        );
        assert_eq!(
            routing.quantizations.as_deref(),
            Some(["fp16".to_string(), "fp8".to_string()].as_slice())
        );
        assert_eq!(
            routing.sort,
            Some(OpenRouterSort::Object {
                by: Some("price".to_string()),
                partition: Some(None),
            })
        );
        let max_price = routing.max_price.as_ref().unwrap();
        assert_eq!(
            max_price.prompt,
            Some(OpenRouterPrice::String("0.5".to_string()))
        );
        assert_eq!(
            max_price.completion,
            Some(OpenRouterPrice::Number(
                serde_json::Number::from_f64(1.5).unwrap()
            ))
        );
        assert_eq!(
            max_price.image,
            Some(OpenRouterPrice::String("2".to_string()))
        );
        assert_eq!(
            max_price.audio,
            Some(OpenRouterPrice::Number(
                serde_json::Number::from_f64(0.25).unwrap()
            ))
        );
        assert_eq!(
            max_price.request,
            Some(OpenRouterPrice::String("0".to_string()))
        );
        assert_eq!(
            routing.preferred_min_throughput,
            Some(OpenRouterNumberOrPercentiles::Percentiles(
                OpenRouterPercentiles {
                    p50: Some(serde_json::Number::from(40)),
                    p75: None,
                    p90: None,
                    p99: Some(serde_json::Number::from(12)),
                }
            ))
        );
        assert_eq!(
            routing.preferred_max_latency,
            Some(OpenRouterNumberOrPercentiles::Number(
                serde_json::Number::from_f64(2.5).unwrap()
            ))
        );
        assert_eq!(
            c.vercel_gateway_routing,
            Some(VercelGatewayRouting {
                only: Some(vec!["bedrock".to_string()]),
                order: Some(vec!["anthropic".to_string(), "openai".to_string()]),
            })
        );
        assert_eq!(c.zai_tool_stream, Some(true));
        assert_eq!(
            c.thinking_token_budget_field,
            Some(ThinkingTokenBudgetField::ThinkingBudgetTokens)
        );
        assert_eq!(c.supports_thinking_token_budget, Some(true));
        assert_eq!(c.supports_openai_grammar_tools, Some(true));
        assert_eq!(c.supports_mid_convo_system_messages, Some(true));
        assert_eq!(c.supports_mid_convo_tool_additions, Some(true));
        assert_eq!(c.supports_strict_mode, Some(false));
        assert_eq!(c.cache_control_format, Some(CacheControlFormat::Anthropic));
        assert_eq!(c.send_session_affinity_headers, Some(true));
        assert_eq!(
            c.session_affinity_format,
            Some(SessionAffinityFormat::OpenaiNosession)
        );
        assert_eq!(c.supports_long_cache_retention, Some(false));
        assert_eq!(c.vllm_priority, Some(serde_json::Number::from(-1)));

        let r = &parsed.responses;
        assert_eq!(r.supports_developer_role, Some(false));
        assert_eq!(r.supports_mid_convo_system_messages, Some(true));
        assert_eq!(
            r.session_affinity_format,
            Some(SessionAffinityFormat::Openrouter)
        );
        assert_eq!(r.supports_long_cache_retention, Some(true));
        assert_eq!(r.supports_strict_mode, Some(true));
        assert_eq!(r.supports_openai_grammar_tools, Some(true));
        assert_eq!(r.supports_additional_tools, Some(true));
        assert_eq!(r.supports_tool_search, Some(false));
        assert_eq!(r.supports_explicit_prompt_cache_mode, Some(true));
        assert_eq!(r.supports_max_output_tokens, Some(false));

        let a = &parsed.anthropic;
        assert_eq!(a.supports_eager_tool_input_streaming, Some(false));
        assert_eq!(a.supports_long_cache_retention, Some(false));
        assert_eq!(a.send_session_affinity_headers, Some(true));
        assert_eq!(
            a.session_affinity_format,
            Some(AnthropicSessionAffinityFormat::Openrouter)
        );
        assert_eq!(a.supports_cache_control_on_tools, Some(false));
        assert_eq!(a.supports_temperature, Some(false));
        assert_eq!(a.force_adaptive_thinking, Some(true));
        assert_eq!(a.allow_empty_signature, Some(true));
        assert_eq!(a.supports_strict_tools, Some(true));
        assert_eq!(a.supports_mid_convo_effort, Some(true));
        assert_eq!(a.supports_mid_convo_system_messages, Some(true));
        assert_eq!(a.supports_mid_convo_tool_changes, Some(true));
        let fallbacks = a.allowed_fallback_models.as_ref().unwrap();
        assert_eq!(fallbacks.len(), 1);
        assert_eq!(fallbacks[0].provider, "anthropic");
        assert_eq!(fallbacks[0].model, "claude-opus-4-6");
        assert_eq!(fallbacks[0].cost.input, 3.0);
        assert_eq!(fallbacks[0].cost.output, 15.0);

        assert_eq!(parsed.bedrock.supports_strict_mode, Some(true));
        assert_eq!(
            parsed.mistral.supports_mid_convo_system_messages,
            Some(true)
        );

        // Byte-pinned compact round-trip: serializing back reproduces the
        // exact wire format (field names, literal values, key order).
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            CATALOG_FIXTURE_COMPACT
        );
        let reparsed: CatalogFixture = serde_json::from_str(CATALOG_FIXTURE_COMPACT).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn empty_compat_structs_omit_all_fields() {
        assert_eq!(
            serde_json::to_string(&OpenAiCompletionsCompat::default()).unwrap(),
            "{}"
        );
        assert_eq!(
            serde_json::to_string(&OpenAiResponsesCompat::default()).unwrap(),
            "{}"
        );
        assert_eq!(
            serde_json::to_string(&AnthropicMessagesCompat::default()).unwrap(),
            "{}"
        );
        assert_eq!(
            serde_json::to_string(&BedrockCompat::default()).unwrap(),
            "{}"
        );
        assert_eq!(
            serde_json::to_string(&MistralConversationsCompat::default()).unwrap(),
            "{}"
        );
        assert_eq!(
            serde_json::to_string(&OpenRouterRouting::default()).unwrap(),
            "{}"
        );
        assert_eq!(
            serde_json::to_string(&VercelGatewayRouting::default()).unwrap(),
            "{}"
        );
        // Missing keys deserialize to None (upstream `undefined`).
        let partial: OpenAiCompletionsCompat =
            serde_json::from_str(r#"{"thinkingFormat":"openai"}"#).unwrap();
        assert_eq!(partial.thinking_format, Some(ThinkingFormat::Openai));
        assert_eq!(partial.max_tokens_field, None);
        assert_eq!(partial.open_router_routing, None);
    }

    #[test]
    fn open_router_sort_and_metric_shapes_round_trip() {
        // Bare string strategy.
        let sort: OpenRouterSort = serde_json::from_str(r#""throughput""#).unwrap();
        assert_eq!(sort, OpenRouterSort::Strategy("throughput".to_string()));
        assert_eq!(serde_json::to_string(&sort).unwrap(), r#""throughput""#);

        // Object without partition: absent stays absent.
        let sort: OpenRouterSort = serde_json::from_str(r#"{"by":"latency"}"#).unwrap();
        assert_eq!(
            sort,
            OpenRouterSort::Object {
                by: Some("latency".to_string()),
                partition: None,
            }
        );
        assert_eq!(serde_json::to_string(&sort).unwrap(), r#"{"by":"latency"}"#);

        // Explicit null partition serializes back as null (upstream `string | null`).
        let sort: OpenRouterSort =
            serde_json::from_str(r#"{"by":"price","partition":null}"#).unwrap();
        assert_eq!(
            sort,
            OpenRouterSort::Object {
                by: Some("price".to_string()),
                partition: Some(None),
            }
        );
        assert_eq!(
            serde_json::to_string(&sort).unwrap(),
            r#"{"by":"price","partition":null}"#
        );

        // Bare number throughput/latency.
        let throughput: OpenRouterNumberOrPercentiles = serde_json::from_str("40").unwrap();
        assert_eq!(
            throughput,
            OpenRouterNumberOrPercentiles::Number(serde_json::Number::from(40))
        );
        assert_eq!(serde_json::to_string(&throughput).unwrap(), "40");

        // Percentile object.
        let latency: OpenRouterNumberOrPercentiles =
            serde_json::from_str(r#"{"p50":1.5,"p99":8}"#).unwrap();
        assert_eq!(
            latency,
            OpenRouterNumberOrPercentiles::Percentiles(OpenRouterPercentiles {
                p50: Some(serde_json::Number::from_f64(1.5).unwrap()),
                p75: None,
                p90: None,
                p99: Some(serde_json::Number::from(8)),
            })
        );
        assert_eq!(
            serde_json::to_string(&latency).unwrap(),
            r#"{"p50":1.5,"p99":8}"#
        );
    }

    #[test]
    fn allowed_fallback_model_fields_are_required() {
        assert!(serde_json::from_str::<AnthropicAllowedFallbackModel>(
            r#"{"provider":"anthropic","model":"claude-opus-4-6"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<AnthropicAllowedFallbackModel>(
            r#"{"provider":"anthropic","cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}}"#
        )
        .is_err());

        let fixture = r#"{"provider":"openrouter","model":"qwen/qwen3-coder","cost":{"input":3,"output":15,"cacheRead":0.3,"cacheWrite":3.75}}"#;
        let fallback: AnthropicAllowedFallbackModel = serde_json::from_str(fixture).unwrap();
        assert_eq!(fallback.provider, "openrouter");
        assert_eq!(fallback.model, "qwen/qwen3-coder");
        assert_eq!(fallback.cost.output, 15.0);
        assert_eq!(serde_json::to_string(&fallback).unwrap(), fixture);
    }
}
