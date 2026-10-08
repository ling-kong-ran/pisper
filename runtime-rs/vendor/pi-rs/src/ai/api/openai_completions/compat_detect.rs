//! Upstream `detectCompat`/`getCompat`
//! (`packages/ai/src/api/openai-completions.ts:1578-1723`): auto-detection of
//! [`OpenAiCompletionsCompat`] defaults from the provider id, baseUrl, and
//! model id, plus the merge of explicit `model.compat` overrides over the
//! detected values.
//!
//! Upstream's `detectCompat` returns a *resolved* compat object (every field
//! upstream marks required in `ResolvedOpenAICompletionsCompat` carries a
//! concrete value; for an unknown URL those values are exactly the defaults
//! documented in the `OpenAICompletionsCompat` comments in
//! `packages/ai/src/types.ts:667-744`). The Rust compat struct is
//! `Option`-based, so detection emits `Some(value)` for every resolved field
//! and `None` only where upstream leaves the field undefined:
//! `thinkingTokenBudgetField` and `vllmPriority` (off by default,
//! types.ts:718, 743) and `cacheControlFormat` except for OpenRouter
//! `anthropic/` models.

use serde::de::DeserializeOwned;
use std::collections::BTreeMap;

use crate::ai::types::compat::{
    CacheControlFormat, MaxTokensField, OpenAiCompletionsCompat, OpenRouterRouting, ThinkingFormat,
    VercelGatewayRouting,
};
use crate::ai::types::primitives::SessionAffinityFormat;

/// URL-only detection: upstream `detectCompat` for a model whose provider id
/// matches no known provider and whose id is not an OpenRouter alias. The
/// task-brief entry point; use
/// [`detect_openai_completions_compat_for_model`] when a full [`Model`]
/// record is available.
///
/// [`Model`]: crate::ai::types::Model
pub fn detect_openai_completions_compat(base_url: &str) -> OpenAiCompletionsCompat {
    detect_openai_completions_compat_for_model("", base_url, "")
}

/// Full port of upstream `detectCompat` (openai-completions.ts:1583-1679):
/// matches the provider id, baseUrl substrings, and (for OpenRouter) the
/// model-id prefix against the provider families, then resolves every compat
/// flag. Empty `provider`/`model_id` strings (as used by the URL-only
/// [`detect_openai_completions_compat`]) match no family, reproducing
/// upstream's URL-only behavior.
pub fn detect_openai_completions_compat_for_model(
    provider: &str,
    base_url: &str,
    model_id: &str,
) -> OpenAiCompletionsCompat {
    // Provider-family flags (openai-completions.ts:1587-1600). Every URL
    // `contains` is case-sensitive upstream except DeepSeek.
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
    let is_deepseek = provider == "deepseek" || base_url.to_lowercase().contains("deepseek.com");

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
    let cache_control_format = if provider == "openrouter" && model_id.starts_with("anthropic/") {
        Some(CacheControlFormat::Anthropic)
    } else {
        None
    };

    OpenAiCompletionsCompat {
        supports_store: Some(!is_non_standard),
        supports_developer_role: Some(
            is_openrouter_developer_role_model || (!is_non_standard && !is_openrouter),
        ),
        supports_reasoning_effort: Some(
            !is_grok
                && !is_zai
                && !is_moonshot
                && !is_together
                && !is_cloudflare_ai_gateway
                && !is_nvidia
                && !is_ant_ling,
        ),
        supports_usage_in_streaming: Some(true),
        supports_finish_reason: Some(true),
        max_tokens_field: Some(if use_max_tokens {
            MaxTokensField::MaxTokens
        } else {
            MaxTokensField::MaxCompletionTokens
        }),
        requires_tool_result_name: Some(false),
        requires_assistant_after_tool_result: Some(false),
        requires_thinking_as_text: Some(false),
        requires_reasoning_content_on_assistant_messages: Some(is_deepseek),
        thinking_format: Some(if is_deepseek {
            ThinkingFormat::Deepseek
        } else if is_zai {
            ThinkingFormat::Zai
        } else if is_together {
            ThinkingFormat::Together
        } else if is_ant_ling {
            ThinkingFormat::AntLing
        } else if is_openrouter {
            ThinkingFormat::Openrouter
        } else {
            ThinkingFormat::Openai
        }),
        chat_template_kwargs: Some(BTreeMap::new()),
        chat_template_args: Some(BTreeMap::new()),
        open_router_routing: Some(OpenRouterRouting::default()),
        vercel_gateway_routing: Some(VercelGatewayRouting::default()),
        zai_tool_stream: Some(false),
        // Off by default; only set through explicit model compat
        // (types.ts:718, 743).
        thinking_token_budget_field: None,
        supports_thinking_token_budget: Some(false),
        supports_openai_grammar_tools: Some(false),
        supports_mid_convo_system_messages: Some(false),
        supports_mid_convo_tool_additions: Some(false),
        // OpenAI compatibility alone does not imply strict JSON-schema tool
        // support (upstream 890f92088/af7359b90): the detected default is
        // false, and generated capable models enable it explicitly via
        // compat.
        supports_strict_mode: Some(false),
        cache_control_format,
        send_session_affinity_headers: Some(is_openrouter),
        session_affinity_format: Some(if is_openrouter {
            SessionAffinityFormat::Openrouter
        } else {
            SessionAffinityFormat::Openai
        }),
        supports_long_cache_retention: Some(
            !(is_together
                || is_cloudflare_workers_ai
                || is_cloudflare_ai_gateway
                || is_nvidia
                || is_ant_ling),
        ),
        vllm_priority: None,
    }
}

/// Merge half of upstream `getCompat` (openai-completions.ts:1685-1723):
/// explicit `model.compat` fields win per-field over the detected values;
/// unset (or JSON `null`) fields keep the detected value.
///
/// `model_compat` is the raw `Model.compat` object. Non-object values are
/// ignored; a key whose value fails to deserialize as the field's type keeps
/// the detected value. Upstream has no runtime validation — a bad value flows
/// into the request and silently fails its feature check — so falling back to
/// the detected default reproduces that graceful degradation (documented
/// deviation).
pub fn merge_compat(
    detected: OpenAiCompletionsCompat,
    model_compat: Option<&serde_json::Value>,
) -> OpenAiCompletionsCompat {
    let Some(serde_json::Value::Object(overrides)) = model_compat else {
        return detected;
    };
    OpenAiCompletionsCompat {
        supports_store: override_field(overrides, "supportsStore", detected.supports_store),
        supports_developer_role: override_field(
            overrides,
            "supportsDeveloperRole",
            detected.supports_developer_role,
        ),
        supports_reasoning_effort: override_field(
            overrides,
            "supportsReasoningEffort",
            detected.supports_reasoning_effort,
        ),
        supports_usage_in_streaming: override_field(
            overrides,
            "supportsUsageInStreaming",
            detected.supports_usage_in_streaming,
        ),
        supports_finish_reason: override_field(
            overrides,
            "supportsFinishReason",
            detected.supports_finish_reason,
        ),
        max_tokens_field: override_field(overrides, "maxTokensField", detected.max_tokens_field),
        requires_tool_result_name: override_field(
            overrides,
            "requiresToolResultName",
            detected.requires_tool_result_name,
        ),
        requires_assistant_after_tool_result: override_field(
            overrides,
            "requiresAssistantAfterToolResult",
            detected.requires_assistant_after_tool_result,
        ),
        requires_thinking_as_text: override_field(
            overrides,
            "requiresThinkingAsText",
            detected.requires_thinking_as_text,
        ),
        requires_reasoning_content_on_assistant_messages: override_field(
            overrides,
            "requiresReasoningContentOnAssistantMessages",
            detected.requires_reasoning_content_on_assistant_messages,
        ),
        thinking_format: override_field(overrides, "thinkingFormat", detected.thinking_format),
        chat_template_kwargs: override_field(
            overrides,
            "chatTemplateKwargs",
            detected.chat_template_kwargs,
        ),
        chat_template_args: override_field(
            overrides,
            "chatTemplateArgs",
            detected.chat_template_args,
        ),
        open_router_routing: override_field(
            overrides,
            "openRouterRouting",
            detected.open_router_routing,
        ),
        vercel_gateway_routing: override_field(
            overrides,
            "vercelGatewayRouting",
            detected.vercel_gateway_routing,
        ),
        zai_tool_stream: override_field(overrides, "zaiToolStream", detected.zai_tool_stream),
        thinking_token_budget_field: override_field(
            overrides,
            "thinkingTokenBudgetField",
            detected.thinking_token_budget_field,
        ),
        supports_thinking_token_budget: override_field(
            overrides,
            "supportsThinkingTokenBudget",
            detected.supports_thinking_token_budget,
        ),
        supports_openai_grammar_tools: override_field(
            overrides,
            "supportsOpenAIGrammarTools",
            detected.supports_openai_grammar_tools,
        ),
        supports_mid_convo_system_messages: override_field(
            overrides,
            "supportsMidConvoSystemMessages",
            detected.supports_mid_convo_system_messages,
        ),
        supports_mid_convo_tool_additions: override_field(
            overrides,
            "supportsMidConvoToolAdditions",
            detected.supports_mid_convo_tool_additions,
        ),
        supports_strict_mode: override_field(
            overrides,
            "supportsStrictMode",
            detected.supports_strict_mode,
        ),
        cache_control_format: override_field(
            overrides,
            "cacheControlFormat",
            detected.cache_control_format,
        ),
        send_session_affinity_headers: override_field(
            overrides,
            "sendSessionAffinityHeaders",
            detected.send_session_affinity_headers,
        ),
        session_affinity_format: override_field(
            overrides,
            "sessionAffinityFormat",
            detected.session_affinity_format,
        ),
        supports_long_cache_retention: override_field(
            overrides,
            "supportsLongCacheRetention",
            detected.supports_long_cache_retention,
        ),
        // Upstream takes vllmPriority from model.compat only
        // (openai-completions.ts:1721); detection never sets it, so the
        // generic merge below is equivalent.
        vllm_priority: override_field(overrides, "vllmPriority", detected.vllm_priority),
    }
}

/// Per-field override: a present, non-null JSON value replaces the detected
/// value when it deserializes cleanly; anything else (absent, `null` —
/// upstream `undefined`/`null` fall through `??` — or a type mismatch)
/// keeps the detected value.
fn override_field<T: DeserializeOwned>(
    overrides: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    detected: Option<T>,
) -> Option<T> {
    match overrides.get(key) {
        Some(value) if !value.is_null() => T::deserialize(value).ok().or(detected),
        _ => detected,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        detect_openai_completions_compat, detect_openai_completions_compat_for_model, merge_compat,
    };
    use crate::ai::types::compat::{
        CacheControlFormat, MaxTokensField, OpenAiCompletionsCompat, OpenRouterRouting,
        ThinkingFormat,
    };
    use crate::ai::types::primitives::{
        ChatTemplateKwargValue, ChatTemplateVariable, SessionAffinityFormat,
        ThinkingTokenBudgetField,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    /// The unknown-URL detection result: every field at its upstream
    /// documented default (types.ts:667-744 comments).
    fn baseline() -> OpenAiCompletionsCompat {
        OpenAiCompletionsCompat {
            supports_store: Some(true),
            supports_developer_role: Some(true),
            supports_reasoning_effort: Some(true),
            supports_usage_in_streaming: Some(true),
            supports_finish_reason: Some(true),
            max_tokens_field: Some(MaxTokensField::MaxCompletionTokens),
            requires_tool_result_name: Some(false),
            requires_assistant_after_tool_result: Some(false),
            requires_thinking_as_text: Some(false),
            requires_reasoning_content_on_assistant_messages: Some(false),
            thinking_format: Some(ThinkingFormat::Openai),
            chat_template_kwargs: Some(BTreeMap::new()),
            chat_template_args: Some(BTreeMap::new()),
            open_router_routing: Some(OpenRouterRouting::default()),
            vercel_gateway_routing: Some(crate::ai::types::compat::VercelGatewayRouting::default()),
            zai_tool_stream: Some(false),
            thinking_token_budget_field: None,
            supports_thinking_token_budget: Some(false),
            supports_openai_grammar_tools: Some(false),
            supports_mid_convo_system_messages: Some(false),
            supports_mid_convo_tool_additions: Some(false),
            supports_strict_mode: Some(false),
            cache_control_format: None,
            send_session_affinity_headers: Some(false),
            session_affinity_format: Some(SessionAffinityFormat::Openai),
            supports_long_cache_retention: Some(true),
            vllm_priority: None,
        }
    }

    #[test]
    fn unknown_url_yields_documented_defaults() {
        assert_eq!(
            detect_openai_completions_compat("https://api.example.com/v1"),
            baseline()
        );
    }

    #[test]
    fn cerebras_url_detection() {
        let detected = detect_openai_completions_compat("https://api.cerebras.ai/v1");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn xai_url_detection() {
        let detected = detect_openai_completions_compat("https://api.x.ai/v1");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_reasoning_effort: Some(false),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn chutes_url_detection() {
        let detected = detect_openai_completions_compat("https://api.chutes.ai/v1");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn deepseek_url_detection() {
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            requires_reasoning_content_on_assistant_messages: Some(true),
            thinking_format: Some(ThinkingFormat::Deepseek),
            ..baseline()
        };
        assert_eq!(
            detect_openai_completions_compat("https://api.deepseek.com/v1"),
            expected
        );
        // Only the DeepSeek match is case-insensitive upstream
        // (`baseUrl.toLowerCase().includes("deepseek.com")`).
        assert_eq!(
            detect_openai_completions_compat("https://api.DeepSeek.com/v1"),
            expected
        );
    }

    #[test]
    fn nvidia_nim_url_detection() {
        let detected = detect_openai_completions_compat("https://integrate.api.nvidia.com/v1");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_reasoning_effort: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            supports_strict_mode: Some(false),
            supports_long_cache_retention: Some(false),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn together_url_detection() {
        let detected = detect_openai_completions_compat("https://api.together.ai/v1");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_reasoning_effort: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            thinking_format: Some(ThinkingFormat::Together),
            supports_strict_mode: Some(false),
            supports_long_cache_retention: Some(false),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn zai_url_detection() {
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_reasoning_effort: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            thinking_format: Some(ThinkingFormat::Zai),
            ..baseline()
        };
        assert_eq!(
            detect_openai_completions_compat("https://api.z.ai/api/coding/paas/v4"),
            expected
        );
        // The zai-coding-cn catalog URL matches via open.bigmodel.cn.
        assert_eq!(
            detect_openai_completions_compat("https://open.bigmodel.cn/api/coding/paas/v4"),
            expected
        );
    }

    #[test]
    fn moonshot_url_detection() {
        let detected = detect_openai_completions_compat("https://api.moonshot.ai/v1");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_reasoning_effort: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            supports_strict_mode: Some(false),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn ant_ling_url_detection() {
        let detected = detect_openai_completions_compat("https://api.ant-ling.com/v1");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_reasoning_effort: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            thinking_format: Some(ThinkingFormat::AntLing),
            supports_long_cache_retention: Some(false),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn opencode_url_detection() {
        // Non-standard (no `store`, no `developer` role) but keeps
        // max_completion_tokens and reasoning_effort.
        let detected = detect_openai_completions_compat("https://opencode.ai/zen/v1");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn cloudflare_workers_ai_url_detection() {
        // Keeps max_completion_tokens and reasoning_effort, but no long cache
        // retention.
        let detected = detect_openai_completions_compat("https://api.cloudflare.com/client/v4");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_long_cache_retention: Some(false),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn cloudflare_ai_gateway_url_detection() {
        let detected =
            detect_openai_completions_compat("https://gateway.ai.cloudflare.com/v1/acct/gw/openai");
        let expected = OpenAiCompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_reasoning_effort: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            supports_strict_mode: Some(false),
            supports_long_cache_retention: Some(false),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn openrouter_url_detection() {
        // OpenRouter is not "non-standard" upstream, so `store` stays
        // supported; the developer role needs an `anthropic/` or `openai/`
        // model-id prefix (absent for URL-only detection).
        let detected = detect_openai_completions_compat("https://openrouter.ai/api/v1");
        let expected = OpenAiCompletionsCompat {
            supports_developer_role: Some(false),
            thinking_format: Some(ThinkingFormat::Openrouter),
            send_session_affinity_headers: Some(true),
            session_affinity_format: Some(SessionAffinityFormat::Openrouter),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn openrouter_anthropic_alias_detection() {
        // Pins upstream openai-completions-cache-control-format.test.ts
        // "preserves Anthropic-style cache markers for OpenRouter Anthropic
        // batch aliases".
        let detected = detect_openai_completions_compat_for_model(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "anthropic/claude-fable-5.1:batch",
        );
        let expected = OpenAiCompletionsCompat {
            supports_developer_role: Some(true),
            thinking_format: Some(ThinkingFormat::Openrouter),
            send_session_affinity_headers: Some(true),
            session_affinity_format: Some(SessionAffinityFormat::Openrouter),
            cache_control_format: Some(CacheControlFormat::Anthropic),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn openrouter_openai_alias_detection() {
        // `openai/` ids get the developer role but no Anthropic cache control.
        let detected = detect_openai_completions_compat_for_model(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "openai/gpt-5.2",
        );
        let expected = OpenAiCompletionsCompat {
            supports_developer_role: Some(true),
            thinking_format: Some(ThinkingFormat::Openrouter),
            send_session_affinity_headers: Some(true),
            session_affinity_format: Some(SessionAffinityFormat::Openrouter),
            ..baseline()
        };
        assert_eq!(detected, expected);
    }

    #[test]
    fn provider_name_detection_matches_url_detection() {
        // Provider ids activate the same families as their URL patterns, so
        // each provider-only match must equal its URL twin (pinned above).
        assert_eq!(
            detect_openai_completions_compat_for_model(
                "deepseek",
                "https://deepseek.example.com/v1",
                ""
            ),
            detect_openai_completions_compat("https://api.deepseek.com/v1")
        );
        assert_eq!(
            detect_openai_completions_compat_for_model(
                "zai-coding-cn",
                "https://zai.example.com/v1",
                ""
            ),
            detect_openai_completions_compat("https://open.bigmodel.cn/api/coding/paas/v4")
        );
        for provider in ["moonshotai", "moonshotai-cn"] {
            assert_eq!(
                detect_openai_completions_compat_for_model(
                    provider,
                    "https://moonshot.example.com/v1",
                    ""
                ),
                detect_openai_completions_compat("https://api.moonshot.ai/v1")
            );
        }
        assert_eq!(
            detect_openai_completions_compat_for_model("xai", "https://grok.example.com/v1", ""),
            detect_openai_completions_compat("https://api.x.ai/v1")
        );
        // cacheControlFormat requires provider == "openrouter" (no URL
        // fallback, openai-completions.ts:1632), so both sides must carry the
        // provider id for the anthropic/ alias to match.
        assert_eq!(
            detect_openai_completions_compat_for_model(
                "openrouter",
                "https://router.example.com/v1",
                "anthropic/x"
            ),
            detect_openai_completions_compat_for_model(
                "openrouter",
                "https://openrouter.ai/api/v1",
                "anthropic/x"
            )
        );
        // URL-only openrouter detection (empty provider) never yields the
        // Anthropic cache-control format.
        assert_eq!(
            detect_openai_completions_compat("https://openrouter.ai/api/v1").cache_control_format,
            None
        );
        assert_eq!(
            detect_openai_completions_compat_for_model(
                "",
                "https://openrouter.ai/api/v1",
                "anthropic/x"
            )
            .cache_control_format,
            None
        );
    }

    #[test]
    fn merge_without_overrides_returns_detected() {
        let detected = detect_openai_completions_compat("https://api.deepseek.com/v1");
        assert_eq!(merge_compat(detected.clone(), None), detected);
        assert_eq!(merge_compat(detected.clone(), Some(&json!(null))), detected);
    }

    #[test]
    fn merge_partial_override_wins_per_field() {
        let detected = detect_openai_completions_compat("https://api.deepseek.com/v1");
        let merged = merge_compat(detected.clone(), Some(&json!({"thinkingFormat": "openai"})));
        let mut expected = detected;
        expected.thinking_format = Some(ThinkingFormat::Openai);
        assert_eq!(merged, expected);
    }

    #[test]
    fn merge_null_override_keeps_detected() {
        let detected = detect_openai_completions_compat("https://api.deepseek.com/v1");
        let merged = merge_compat(
            detected.clone(),
            Some(&json!({"thinkingFormat": null, "maxTokensField": null})),
        );
        assert_eq!(merged, detected);
    }

    #[test]
    fn merge_invalid_override_value_keeps_detected() {
        let detected = detect_openai_completions_compat("https://api.deepseek.com/v1");
        let merged = merge_compat(
            detected.clone(),
            Some(&json!({"maxTokensField": "bogus", "supportsStore": "yes"})),
        );
        assert_eq!(merged, detected);
    }

    #[test]
    fn merge_unknown_keys_are_ignored() {
        let detected = detect_openai_completions_compat("https://api.example.com/v1");
        let merged = merge_compat(detected.clone(), Some(&json!({"bogus": true})));
        assert_eq!(merged, detected);
    }

    #[test]
    fn merge_vllm_priority_comes_only_from_compat() {
        // Detection never sets vllmPriority (upstream leaves it off the
        // detected object); an explicit compat value is the only source.
        let detected = detect_openai_completions_compat("https://api.example.com/v1");
        assert_eq!(detected.vllm_priority, None);
        let merged = merge_compat(detected, Some(&json!({"vllmPriority": -1})));
        assert_eq!(merged.vllm_priority, Some(serde_json::Number::from(-1)));
    }

    #[test]
    fn merge_cache_control_format_override() {
        // Mirrors upstream openai-completions-cache-control-format.test.ts
        // "applies Anthropic-style cache markers when model compat enables
        // them": an explicit compat entry on any openai-completions model.
        let detected = detect_openai_completions_compat_for_model(
            "openrouter",
            "https://openrouter.ai/api/v1",
            "openai/gpt-5.2",
        );
        assert_eq!(detected.cache_control_format, None);
        let merged = merge_compat(detected, Some(&json!({"cacheControlFormat": "anthropic"})));
        assert_eq!(
            merged.cache_control_format,
            Some(CacheControlFormat::Anthropic)
        );
    }

    #[test]
    fn merge_nested_open_router_routing_override() {
        let detected = detect_openai_completions_compat("https://api.example.com/v1");
        let merged = merge_compat(
            detected,
            Some(&json!({"openRouterRouting": {"allow_fallbacks": false}})),
        );
        assert_eq!(
            merged.open_router_routing,
            Some(OpenRouterRouting {
                allow_fallbacks: Some(false),
                ..Default::default()
            })
        );
    }

    #[test]
    fn merge_non_object_compat_is_ignored() {
        let detected = detect_openai_completions_compat("https://api.example.com/v1");
        assert_eq!(
            merge_compat(detected.clone(), Some(&json!("openai"))),
            detected
        );
        assert_eq!(
            merge_compat(detected.clone(), Some(&json!([1, 2]))),
            detected
        );
    }

    #[test]
    fn merge_applies_multiple_overrides_together() {
        let detected = detect_openai_completions_compat("https://api.chutes.ai/v1");
        let merged = merge_compat(
            detected,
            Some(&json!({
                "supportsStore": true,
                "thinkingFormat": "qwen-chat-template",
                "thinkingTokenBudgetField": "thinking_budget",
                "chatTemplateKwargs": {"enable_thinking": {"$var": "thinking.enabled"}}
            })),
        );
        assert_eq!(merged.supports_store, Some(true));
        assert_eq!(
            merged.thinking_format,
            Some(ThinkingFormat::QwenChatTemplate)
        );
        assert_eq!(
            merged.thinking_token_budget_field,
            Some(ThinkingTokenBudgetField::ThinkingBudget)
        );
        let kwargs = merged.chat_template_kwargs.as_ref().unwrap();
        assert_eq!(
            kwargs.get("enable_thinking"),
            Some(&ChatTemplateKwargValue::Variable {
                var: ChatTemplateVariable::ThinkingEnabled,
                omit_when_off: None,
            })
        );
        // Untouched detected fields survive the merge.
        assert_eq!(merged.max_tokens_field, Some(MaxTokensField::MaxTokens));
        assert_eq!(merged.supports_developer_role, Some(false));
    }
}
