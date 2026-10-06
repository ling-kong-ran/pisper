//! Primitive shared types from upstream `packages/ai/src/types.ts`:
//! known APIs/providers, thinking-level unions, usage/cost accounting, and stop
//! reasons. Wire format (serde JSON) must match the TypeScript unions' literal
//! string values byte-for-byte (spec section 2).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Upstream `KnownApi` (types.ts:17-27): the ten provider APIs pi ships adapters for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KnownApi {
    OpenaiCompletions,
    MistralConversations,
    OpenaiResponses,
    AzureOpenaiResponses,
    OpenaiCodexResponses,
    AnthropicMessages,
    BedrockConverseStream,
    GoogleGenerativeAi,
    GoogleVertex,
    PiMessages,
}

pub const OPENAI_COMPLETIONS: KnownApi = KnownApi::OpenaiCompletions;
pub const MISTRAL_CONVERSATIONS: KnownApi = KnownApi::MistralConversations;
pub const OPENAI_RESPONSES: KnownApi = KnownApi::OpenaiResponses;
pub const AZURE_OPENAI_RESPONSES: KnownApi = KnownApi::AzureOpenaiResponses;
pub const OPENAI_CODEX_RESPONSES: KnownApi = KnownApi::OpenaiCodexResponses;
pub const ANTHROPIC_MESSAGES: KnownApi = KnownApi::AnthropicMessages;
pub const BEDROCK_CONVERSE_STREAM: KnownApi = KnownApi::BedrockConverseStream;
pub const GOOGLE_GENERATIVE_AI: KnownApi = KnownApi::GoogleGenerativeAi;
pub const GOOGLE_VERTEX: KnownApi = KnownApi::GoogleVertex;
pub const PI_MESSAGES: KnownApi = KnownApi::PiMessages;

/// All known APIs, in upstream declaration order (types.ts:18-27).
pub const KNOWN_API: &[KnownApi] = &[
    OPENAI_COMPLETIONS,
    MISTRAL_CONVERSATIONS,
    OPENAI_RESPONSES,
    AZURE_OPENAI_RESPONSES,
    OPENAI_CODEX_RESPONSES,
    ANTHROPIC_MESSAGES,
    BEDROCK_CONVERSE_STREAM,
    GOOGLE_GENERATIVE_AI,
    GOOGLE_VERTEX,
    PI_MESSAGES,
];

/// Upstream `KnownProvider` ids (types.ts:36-75), verbatim and in upstream order.
/// Provider ids are open-ended upstream (`KnownProvider | string`), so ids stay
/// plain strings checked against this list.
pub const KNOWN_PROVIDERS: &[&str] = &[
    "amazon-bedrock",
    "ant-ling",
    "anthropic",
    "google",
    "google-vertex",
    "openai",
    "azure-openai-responses",
    "openai-codex",
    "radius",
    "nvidia",
    "deepseek",
    "github-copilot",
    "xai",
    "groq",
    "cerebras",
    "openrouter",
    "vercel-ai-gateway",
    "zai",
    "zai-coding-cn",
    "mistral",
    "minimax",
    "minimax-cn",
    "moonshotai",
    "moonshotai-cn",
    "huggingface",
    "fireworks",
    "together",
    "baseten",
    "opencode",
    "opencode-go",
    "kimi-coding",
    "cloudflare-workers-ai",
    "cloudflare-ai-gateway",
    "qwen-token-plan",
    "qwen-token-plan-cn",
    "qwen-token-plan-individual",
    "xiaomi",
    "xiaomi-token-plan-cn",
    "xiaomi-token-plan-ams",
    "xiaomi-token-plan-sgp",
];

/// Upstream `ToolChoice` (types.ts:82).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
}

/// Upstream `ThinkingLevel` (types.ts:83).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Upstream `ModelThinkingLevel` (types.ts:84): `ThinkingLevel` plus `"off"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Upstream `ThinkingLevelMap` (types.ts:85): `Partial<Record<ModelThinkingLevel, string | null>>`.
/// Absent keys fall back to provider defaults; `null` marks a level as unsupported.
/// A `BTreeMap` (not a `HashMap`) so iteration and serialized key order are
/// deterministic — the wire shape is unchanged, only key ordering is pinned.
pub type ThinkingLevelMap = BTreeMap<String, Option<String>>;

/// Upstream `SamplingParams` (types.ts:88): `Record<string, unknown>`.
/// A `BTreeMap` carrier like [`ThinkingLevelMap`]: the wire stays a
/// string-keyed JSON object; only key ordering is pinned.
pub type SamplingParams = BTreeMap<String, serde_json::Value>;

/// Upstream `SamplingParamsByThinkingLevel` (types.ts:89):
/// `Partial<Record<ModelThinkingLevel, SamplingParams>>`. Keys are the
/// upstream `ModelThinkingLevel` wire strings; unknown keys round-trip
/// (upstream applies no runtime validation to the partial record).
pub type SamplingParamsByThinkingLevel = BTreeMap<String, SamplingParams>;

/// Upstream `ChatTemplateKwargValue` (types.ts:86-94): a literal value or a
/// `$var` reference resolved by providers that expand chat-template kwargs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatTemplateKwargValue {
    Null,
    Bool(bool),
    /// `serde_json::Number` keeps integers integer on reserialization, matching
    /// upstream JSON (an `f64` would emit `42.0` for upstream's `42`).
    /// Port-safer difference, kept deliberately: `Number` equality is
    /// representation-sensitive (`1` ≠ `1.0`), where JS numeric equality
    /// treats them as one value.
    Number(serde_json::Number),
    String(String),
    Variable {
        #[serde(rename = "$var")]
        var: ChatTemplateVariable,
        #[serde(rename = "omitWhenOff", skip_serializing_if = "Option::is_none")]
        omit_when_off: Option<bool>,
    },
}

/// Allowed `$var` references in `ChatTemplateKwargValue::Variable` (types.ts:92).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChatTemplateVariable {
    #[serde(rename = "thinking.enabled")]
    ThinkingEnabled,
    #[serde(rename = "thinking.effort")]
    ThinkingEffort,
    #[serde(rename = "thinking.budget")]
    ThinkingBudget,
}

/// Upstream `ThinkingTokenBudgetField` (types.ts:97): top-level request field used
/// to cap reasoning tokens on OpenAI-compatible servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingTokenBudgetField {
    ThinkingTokenBudget,
    ThinkingBudget,
    ThinkingBudgetTokens,
}

/// Upstream `ThinkingBudgets` (types.ts:100-105): token budgets per thinking level
/// for token-based providers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingBudgets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimal: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub medium: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<u32>,
}

/// Upstream `CacheRetention` (types.ts:108): prompt cache retention preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

/// Upstream `Transport` (types.ts:110): preferred transport for providers that
/// support multiple transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Sse,
    Websocket,
    #[serde(rename = "websocket-cached")]
    WebsocketCached,
    Auto,
}

/// Upstream `SessionAffinityFormat` (types.ts:116): how session ids are formatted
/// for providers with session-based caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionAffinityFormat {
    Openai,
    OpenaiNosession,
    Openrouter,
}

/// Upstream `StopReason` (types.ts:412). Serializes with the literal upstream union
/// values, notably `"toolUse"` (not snake_case).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Pending,
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Deferred,
}

/// Serde helpers for cost amounts (controller ruling, Task 10): upstream
/// TypeScript serializes dollar amounts through `JSON.stringify`, which emits
/// integral numbers without a decimal point (`0`, never `0.0`), and re-serializing
/// a parsed `0.0` also yields `0`. serde_json's default f64 encoding (ryu)
/// always writes the decimal point, so an integral cost would round-trip to
/// different bytes than upstream produced. Serialize integral values as JSON
/// integers and pass everything else to the default f64 encoding;
/// deserialization is the standard f64 behavior (`0` and `0.0` both parse to
/// `0.0`, like `JSON.parse`).
/// JS-number wire format for classifier floats: whole-number f64s emit as
/// integers (JS `1`, never `1.0`); everything else takes the shortest f64
/// form. The module is `pub(crate)` for the classifier types.
pub(crate) mod js_number {
    pub(crate) use super::cost_amount::deserialize;
    pub(crate) use super::cost_amount::serialize;
}

pub(crate) mod cost_amount {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        if value.is_finite() && value.fract() == 0.0 && value.abs() <= i64::MAX as f64 {
            serializer.serialize_i64(*value as i64)
        } else {
            serializer.serialize_f64(*value)
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        f64::deserialize(deserializer)
    }
}

/// Upstream `Usage.cost` (types.ts:403-409): dollar amounts for the request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
    #[serde(with = "cost_amount")]
    pub input: f64,
    #[serde(with = "cost_amount")]
    pub output: f64,
    #[serde(with = "cost_amount")]
    pub cache_read: f64,
    #[serde(with = "cost_amount")]
    pub cache_write: f64,
    #[serde(with = "cost_amount")]
    pub total: f64,
}

/// Upstream `Usage` (types.ts:389-410). Token counts are integers; `cacheWrite1h`
/// and `reasoning` are omitted from JSON when absent, like upstream `undefined`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Subset of `cache_write` written with 1h retention (Anthropic only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<u64>,
    /// Reasoning/thinking tokens; a subset of `output` when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    pub total_tokens: u64,
    pub cost: UsageCost,
}

/// Upstream `ModelCostRates` (types.ts:934-939): dollars per million tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostRates {
    #[serde(with = "cost_amount")]
    pub input: f64,
    #[serde(with = "cost_amount")]
    pub output: f64,
    #[serde(with = "cost_amount")]
    pub cache_read: f64,
    #[serde(with = "cost_amount")]
    pub cache_write: f64,
}

/// Upstream `ModelCostTier` (types.ts:941-944): rates for requests whose total
/// input usage exceeds `input_tokens_above`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostTier {
    #[serde(with = "cost_amount")]
    pub input: f64,
    #[serde(with = "cost_amount")]
    pub output: f64,
    #[serde(with = "cost_amount")]
    pub cache_read: f64,
    #[serde(with = "cost_amount")]
    pub cache_write: f64,
    pub input_tokens_above: u64,
}

/// Upstream `ModelCost` (types.ts:946-949): base rates plus optional request-wide
/// pricing tiers; the highest matching input threshold applies to the full request.
/// Upstream defines it as `ModelCostRates & { tiers?: ... }`; the rate fields are
/// duplicated verbatim and use the same integral-preserving cost serializer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    #[serde(with = "cost_amount")]
    pub input: f64,
    #[serde(with = "cost_amount")]
    pub output: f64,
    #[serde(with = "cost_amount")]
    pub cache_read: f64,
    #[serde(with = "cost_amount")]
    pub cache_write: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<ModelCostTier>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reason_wire_values_match_upstream() {
        let cases = [
            (StopReason::Pending, "\"pending\""),
            (StopReason::Stop, "\"stop\""),
            (StopReason::Length, "\"length\""),
            (StopReason::ToolUse, "\"toolUse\""),
            (StopReason::Error, "\"error\""),
            (StopReason::Aborted, "\"aborted\""),
            (StopReason::Deferred, "\"deferred\""),
        ];
        for (reason, wire) in cases {
            assert_eq!(serde_json::to_string(&reason).unwrap(), wire);
            let back: StopReason = serde_json::from_str(wire).unwrap();
            assert_eq!(back, reason);
        }
    }

    #[test]
    fn usage_round_trips_fixture_bytes() {
        let fixture = r#"{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{"input":0.01,"output":0.02,"cacheRead":0,"cacheWrite":0,"total":0.03}}"#;
        let usage: Usage = serde_json::from_str(fixture).unwrap();
        assert_eq!(usage.input, 10);
        assert_eq!(usage.output, 5);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_write, 0);
        assert_eq!(usage.cache_write_1h, None);
        assert_eq!(usage.reasoning, None);
        assert_eq!(usage.total_tokens, 15);
        assert_eq!(usage.cost.input, 0.01);
        assert_eq!(usage.cost.output, 0.02);
        assert_eq!(usage.cost.cache_read, 0.0);
        assert_eq!(usage.cost.cache_write, 0.0);
        assert_eq!(usage.cost.total, 0.03);
        assert_eq!(serde_json::to_string(&usage).unwrap(), fixture);
    }

    #[test]
    fn usage_optional_fields_round_trip_when_present() {
        let fixture = r#"{"input":1,"output":2,"cacheRead":0,"cacheWrite":3,"cacheWrite1h":3,"reasoning":2,"totalTokens":6,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}"#;
        let usage: Usage = serde_json::from_str(fixture).unwrap();
        assert_eq!(usage.cache_write_1h, Some(3));
        assert_eq!(usage.reasoning, Some(2));
        assert_eq!(serde_json::to_string(&usage).unwrap(), fixture);
    }

    #[test]
    fn cost_amounts_emit_integral_values_without_decimal_point() {
        // Upstream JSON.stringify(0) is `0`, never `0.0`; parsed 0.0 also
        // re-serializes as `0`. Non-integral amounts keep their decimals.
        let cost = UsageCost::default();
        assert_eq!(
            serde_json::to_string(&cost).unwrap(),
            r#"{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}"#
        );
        let parsed: UsageCost = serde_json::from_str(
            r#"{"input":0.0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}"#,
        )
        .unwrap();
        assert_eq!(parsed.input, 0.0);
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            r#"{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}"#
        );
        let fractional: UsageCost = serde_json::from_str(
            r#"{"input":0.01,"output":1.5,"cacheRead":0,"cacheWrite":0,"total":0}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&fractional).unwrap(),
            r#"{"input":0.01,"output":1.5,"cacheRead":0,"cacheWrite":0,"total":0}"#
        );
    }

    #[test]
    fn usage_omits_none_optional_fields() {
        let usage = Usage {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 15,
            cost: UsageCost::default(),
        };
        let json = serde_json::to_string(&usage).unwrap();
        assert!(!json.contains("cacheWrite1h"), "{json}");
        assert!(!json.contains("reasoning"), "{json}");
    }

    #[test]
    fn known_api_matches_upstream_wire_names_and_order() {
        let expected = [
            "openai-completions",
            "mistral-conversations",
            "openai-responses",
            "azure-openai-responses",
            "openai-codex-responses",
            "anthropic-messages",
            "bedrock-converse-stream",
            "google-generative-ai",
            "google-vertex",
            "pi-messages",
        ];
        assert_eq!(KNOWN_API.len(), expected.len());
        for (api, wire) in KNOWN_API.iter().zip(expected) {
            let quoted = format!("\"{wire}\"");
            assert_eq!(serde_json::to_string(api).unwrap(), quoted);
            let back: KnownApi = serde_json::from_str(&quoted).unwrap();
            assert_eq!(back, *api);
        }
    }

    #[test]
    fn known_api_consts_cover_all_variants() {
        assert_eq!(OPENAI_COMPLETIONS, KnownApi::OpenaiCompletions);
        assert_eq!(MISTRAL_CONVERSATIONS, KnownApi::MistralConversations);
        assert_eq!(OPENAI_RESPONSES, KnownApi::OpenaiResponses);
        assert_eq!(AZURE_OPENAI_RESPONSES, KnownApi::AzureOpenaiResponses);
        assert_eq!(OPENAI_CODEX_RESPONSES, KnownApi::OpenaiCodexResponses);
        assert_eq!(ANTHROPIC_MESSAGES, KnownApi::AnthropicMessages);
        assert_eq!(BEDROCK_CONVERSE_STREAM, KnownApi::BedrockConverseStream);
        assert_eq!(GOOGLE_GENERATIVE_AI, KnownApi::GoogleGenerativeAi);
        assert_eq!(GOOGLE_VERTEX, KnownApi::GoogleVertex);
        assert_eq!(PI_MESSAGES, KnownApi::PiMessages);
    }

    #[test]
    fn known_providers_match_upstream_list_verbatim() {
        let expected = [
            "amazon-bedrock",
            "ant-ling",
            "anthropic",
            "google",
            "google-vertex",
            "openai",
            "azure-openai-responses",
            "openai-codex",
            "radius",
            "nvidia",
            "deepseek",
            "github-copilot",
            "xai",
            "groq",
            "cerebras",
            "openrouter",
            "vercel-ai-gateway",
            "zai",
            "zai-coding-cn",
            "mistral",
            "minimax",
            "minimax-cn",
            "moonshotai",
            "moonshotai-cn",
            "huggingface",
            "fireworks",
            "together",
            "baseten",
            "opencode",
            "opencode-go",
            "kimi-coding",
            "cloudflare-workers-ai",
            "cloudflare-ai-gateway",
            "qwen-token-plan",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
            "xiaomi",
            "xiaomi-token-plan-cn",
            "xiaomi-token-plan-ams",
            "xiaomi-token-plan-sgp",
        ];
        assert_eq!(KNOWN_PROVIDERS.len(), expected.len());
        assert_eq!(KNOWN_PROVIDERS, expected);
    }

    #[test]
    fn union_wire_values_match_upstream() {
        let tool_choice = [
            (ToolChoice::Auto, "\"auto\""),
            (ToolChoice::None, "\"none\""),
        ];
        let thinking_level = [
            (ThinkingLevel::Minimal, "\"minimal\""),
            (ThinkingLevel::Low, "\"low\""),
            (ThinkingLevel::Medium, "\"medium\""),
            (ThinkingLevel::High, "\"high\""),
            (ThinkingLevel::Xhigh, "\"xhigh\""),
            (ThinkingLevel::Max, "\"max\""),
        ];
        let model_thinking_level = [
            (ModelThinkingLevel::Off, "\"off\""),
            (ModelThinkingLevel::Minimal, "\"minimal\""),
            (ModelThinkingLevel::Low, "\"low\""),
            (ModelThinkingLevel::Medium, "\"medium\""),
            (ModelThinkingLevel::High, "\"high\""),
            (ModelThinkingLevel::Xhigh, "\"xhigh\""),
            (ModelThinkingLevel::Max, "\"max\""),
        ];
        let cache_retention = [
            (CacheRetention::None, "\"none\""),
            (CacheRetention::Short, "\"short\""),
            (CacheRetention::Long, "\"long\""),
        ];
        let transport = [
            (Transport::Sse, "\"sse\""),
            (Transport::Websocket, "\"websocket\""),
            (Transport::WebsocketCached, "\"websocket-cached\""),
            (Transport::Auto, "\"auto\""),
        ];
        let session_affinity = [
            (SessionAffinityFormat::Openai, "\"openai\""),
            (
                SessionAffinityFormat::OpenaiNosession,
                "\"openai-nosession\"",
            ),
            (SessionAffinityFormat::Openrouter, "\"openrouter\""),
        ];
        let token_budget_field = [
            (
                ThinkingTokenBudgetField::ThinkingTokenBudget,
                "\"thinking_token_budget\"",
            ),
            (
                ThinkingTokenBudgetField::ThinkingBudget,
                "\"thinking_budget\"",
            ),
            (
                ThinkingTokenBudgetField::ThinkingBudgetTokens,
                "\"thinking_budget_tokens\"",
            ),
        ];
        for (value, wire) in tool_choice {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(serde_json::from_str::<ToolChoice>(wire).unwrap(), value);
        }
        for (value, wire) in thinking_level {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(serde_json::from_str::<ThinkingLevel>(wire).unwrap(), value);
        }
        for (value, wire) in model_thinking_level {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(
                serde_json::from_str::<ModelThinkingLevel>(wire).unwrap(),
                value
            );
        }
        for (value, wire) in cache_retention {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(serde_json::from_str::<CacheRetention>(wire).unwrap(), value);
        }
        for (value, wire) in transport {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(serde_json::from_str::<Transport>(wire).unwrap(), value);
        }
        for (value, wire) in session_affinity {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(
                serde_json::from_str::<SessionAffinityFormat>(wire).unwrap(),
                value
            );
        }
        for (value, wire) in token_budget_field {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(
                serde_json::from_str::<ThinkingTokenBudgetField>(wire).unwrap(),
                value
            );
        }
    }

    #[test]
    fn thinking_budgets_omits_none_and_round_trips() {
        let fixture = r#"{"low":1024,"high":8192}"#;
        let budgets: ThinkingBudgets = serde_json::from_str(fixture).unwrap();
        assert_eq!(budgets.minimal, None);
        assert_eq!(budgets.low, Some(1024));
        assert_eq!(budgets.medium, None);
        assert_eq!(budgets.high, Some(8192));
        assert_eq!(serde_json::to_string(&budgets).unwrap(), fixture);
        assert_eq!(
            serde_json::to_string(&ThinkingBudgets::default()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn thinking_level_map_round_trips_nulls_and_values() {
        let fixture = r#"{"off":null,"medium":"enabled"}"#;
        let map: ThinkingLevelMap = serde_json::from_str(fixture).unwrap();
        assert_eq!(map.get("off"), Some(&None));
        assert_eq!(map.get("medium"), Some(&Some("enabled".to_string())));
        assert!(!map.contains_key("high"));
        // BTreeMap pins the key order: serialization is deterministic and
        // sorted regardless of the JSON's original key order.
        let unordered = r#"{"medium":"enabled","off":null,"high":"thinking"}"#;
        let map: ThinkingLevelMap = serde_json::from_str(unordered).unwrap();
        assert_eq!(
            serde_json::to_string(&map).unwrap(),
            r#"{"high":"thinking","medium":"enabled","off":null}"#
        );
        let back: ThinkingLevelMap =
            serde_json::from_str(&serde_json::to_string(&map).unwrap()).unwrap();
        assert_eq!(back, map);
    }

    #[test]
    fn model_cost_with_tiers_round_trips() {
        let fixture = r#"{"input":3,"output":15,"cacheRead":0.3,"cacheWrite":3.75,"tiers":[{"input":1.5,"output":7.5,"cacheRead":0.15,"cacheWrite":1.875,"inputTokensAbove":200000}]}"#;
        let cost: ModelCost = serde_json::from_str(fixture).unwrap();
        assert_eq!(cost.input, 3.0);
        assert_eq!(cost.output, 15.0);
        assert_eq!(cost.cache_read, 0.3);
        assert_eq!(cost.cache_write, 3.75);
        let tiers = cost.tiers.as_ref().unwrap();
        assert_eq!(tiers.len(), 1);
        assert_eq!(tiers[0].input, 1.5);
        assert_eq!(tiers[0].input_tokens_above, 200000);
        assert_eq!(serde_json::to_string(&cost).unwrap(), fixture);
    }

    #[test]
    fn model_cost_omits_none_tiers() {
        let cost = ModelCost::default();
        assert_eq!(
            serde_json::to_string(&cost).unwrap(),
            "{\"input\":0,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0}"
        );
    }

    #[test]
    fn chat_template_kwarg_value_round_trips_all_shapes() {
        let cases: Vec<(&str, ChatTemplateKwargValue)> = vec![
            (r#""auto""#, ChatTemplateKwargValue::String("auto".into())),
            ("true", ChatTemplateKwargValue::Bool(true)),
            ("false", ChatTemplateKwargValue::Bool(false)),
            ("null", ChatTemplateKwargValue::Null),
            (
                "42",
                ChatTemplateKwargValue::Number(serde_json::Number::from(42)),
            ),
            (
                "0.5",
                ChatTemplateKwargValue::Number(serde_json::Number::from_f64(0.5).unwrap()),
            ),
            (
                r#"{"$var":"thinking.budget"}"#,
                ChatTemplateKwargValue::Variable {
                    var: ChatTemplateVariable::ThinkingBudget,
                    omit_when_off: None,
                },
            ),
            (
                r#"{"$var":"thinking.enabled","omitWhenOff":true}"#,
                ChatTemplateKwargValue::Variable {
                    var: ChatTemplateVariable::ThinkingEnabled,
                    omit_when_off: Some(true),
                },
            ),
            (
                r#"{"$var":"thinking.effort","omitWhenOff":false}"#,
                ChatTemplateKwargValue::Variable {
                    var: ChatTemplateVariable::ThinkingEffort,
                    omit_when_off: Some(false),
                },
            ),
        ];
        for (wire, value) in cases {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            let back: ChatTemplateKwargValue = serde_json::from_str(wire).unwrap();
            assert_eq!(back, value);
        }
    }
}
