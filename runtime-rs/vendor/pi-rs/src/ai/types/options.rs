//! Request options from upstream `packages/ai/src/types.ts`:
//! `ProviderRequestOptions` (124-177), `StreamOptions` (179-223),
//! `SimpleStreamOptions` (317-326), and `DeferredHandle` (462-472), plus the
//! provider env/headers aliases (113-114).
//!
//! Wire format must match the TypeScript interfaces field-for-field: struct
//! fields serialize with the upstream camelCase names, optional fields are
//! omitted from JSON when `None` (like upstream `undefined`), and map-valued
//! fields are string-keyed JSON objects (`BTreeMap` here for deterministic
//! key order, `serde_json::Value` for open-ended values).
//!
//! Process-local request callbacks now live in `RequestCallbacks` and are
//! skipped by serde. Models capability-checks adapters rather than silently
//! discarding callbacks. Remaining omitted options:
//! - `telemetryContext` (types.ts:127): telemetry is not ported in M2a.
//! - `fetch` (types.ts:134): Rust uses `reqwest` as the HTTP client; there is
//!   no injectable fetch function. Revisit only if an adapter needs one.
//!
//! Upstream's `ProviderStreamOptions` (types.ts:225, an index-signature
//! intersection) has no runtime existence and no Rust equivalent in M2a;
//! `DeferredFetchOptions`/`DeferredCancelOptions` (types.ts:227-236) land
//! with the M2b deferred stream functions.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

use super::primitives::{CacheRetention, ThinkingBudgets, ThinkingLevel, ToolChoice, Transport};

/// Upstream `ProviderEnv` (types.ts:113): provider-scoped environment
/// overrides; values take precedence over process env for provider
/// configuration (regional settings, endpoint placeholders, proxy variables).
pub type ProviderEnv = BTreeMap<String, String>;

/// Upstream `ProviderHeaders` (types.ts:114): custom HTTP headers merged over
/// provider defaults (caller values win). A `None` value (upstream `null`)
/// suppresses a provider/API default header with the same name.
pub type ProviderHeaders = BTreeMap<String, Option<String>>;

/// Upstream `ProviderRequestOptions` (types.ts:124-177): authentication, HTTP
/// transport tuning, and provider-scoped overrides shared by all provider
/// requests. `onPayload`/`onResponse` are process-local `callbacks`, skipped
/// by serde. The remaining `telemetryContext`/`fetch` omissions and adapter
/// capability boundaries are documented on the module.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRequestOptions {
    /// Process-local lifecycle callbacks; never part of the wire representation.
    #[serde(skip)]
    pub callbacks: super::request_callbacks::RequestCallbacks,
    /// Request cancellation (types.ts:125, upstream `AbortSignal`). The port
    /// carries a [`CancellationToken`]; `None` (upstream `undefined`) runs
    /// unabortable. Transport-only: never serialized or deserialized, so no
    /// wire format carries it.
    #[serde(skip)]
    pub signal: Option<CancellationToken>,
    /// Explicit credential override (types.ts:128).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Provider-scoped environment overrides (types.ts:140).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<ProviderEnv>,
    /// Custom HTTP headers; `None` values suppress provider defaults
    /// (types.ts:158).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<ProviderHeaders>,
    /// HTTP request timeout in milliseconds for providers/SDKs that support
    /// it (types.ts:163; SDK clients default to 10 minutes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Maximum retry attempts for providers/SDKs that support client-side
    /// retries (types.ts:168; SDK clients default to 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Cap on server-requested retry delays in milliseconds; a delay beyond
    /// the cap fails the request so higher-level retry logic can handle it
    /// (types.ts:176; default 60000, `0` disables the cap).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retry_delay_ms: Option<u64>,
}

/// Upstream `StreamOptions` (types.ts:179-223): options for streaming provider
/// requests. Upstream extends `ProviderRequestOptions` via interface
/// inheritance; Rust has no inheritance, so the plan flattens the hierarchy:
/// this is one struct carrying all of the fields — the six provider-request
/// fields (duplicated verbatim from [`ProviderRequestOptions`], deliberately;
/// keep the two field sets in sync) followed by the stream-specific fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamOptions {
    /// Process-local lifecycle callbacks; never part of the wire representation.
    #[serde(skip)]
    pub callbacks: super::request_callbacks::RequestCallbacks,
    // ---- Upstream `ProviderRequestOptions` (types.ts:124-177). ----
    /// Request cancellation (types.ts:125, upstream `AbortSignal`): aborts
    /// the request setup (before `Start`, not retried) and the mid-stream
    /// body reads, settling the message with `stopReason: "aborted"`.
    /// Transport-only: `#[serde(skip)]`, never part of any wire format.
    #[serde(skip)]
    pub signal: Option<CancellationToken>,
    /// Explicit credential override (types.ts:128).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Provider-scoped environment overrides (types.ts:140).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<ProviderEnv>,
    /// Custom HTTP headers; `None` values suppress provider defaults
    /// (types.ts:158).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<ProviderHeaders>,
    /// HTTP request timeout in milliseconds (types.ts:163).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Maximum client-side retry attempts (types.ts:168).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Cap on server-requested retry delays in milliseconds (types.ts:176).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retry_delay_ms: Option<u64>,
    // ---- Upstream `StreamOptions` (types.ts:179-223). ----
    /// Sampling temperature (types.ts:185).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Arbitrary sampling parameters merged into the request body as-is,
    /// after the named request fields, so keys here override them
    /// (types.ts:209, `StreamOptions.samplingParams` re-typed to the
    /// `SamplingParams` alias). Lets custom OpenAI-compatible servers
    /// (llama.cpp, vLLM, SGLang, ...) receive parameters pi does not model.
    /// Merged over `Model.samplingParams`/`samplingParamsByThinkingLevel`
    /// per key; only applied by OpenAI-compatible adapters (completions,
    /// responses, Azure responses) — other APIs ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling_params: Option<super::primitives::SamplingParams>,
    /// Maximum output tokens (types.ts:194).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Preferred transport for providers that support multiple transports;
    /// ignored by providers that do not (types.ts:199).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
    /// Prompt cache retention preference (types.ts:204; default `"short"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_retention: Option<CacheRetention>,
    /// Session identifier for providers that support session-based caching;
    /// enables prompt caching, request routing, or other session-aware
    /// features (types.ts:210). Ignored by providers that do not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// WebSocket connect (open handshake) timeout in milliseconds; stream
    /// idleness after connection uses `timeoutMs` (types.ts:216).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websocket_connect_timeout_ms: Option<u64>,
    /// Optional metadata included in API requests; providers extract the
    /// fields they understand and ignore the rest (types.ts:222; e.g.
    /// Anthropic uses `user_id` for abuse tracking and rate limiting).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
}

/// Upstream `SimpleStreamOptions` (types.ts:317-326): unified options for
/// `streamSimple()`/`completeSimple()`. Upstream extends `StreamOptions`;
/// here the base is embedded and `#[serde(flatten)]`-ed so the wire stays one
/// flat object with the upstream camelCase keys while the fifteen base
/// fields are defined once on [`StreamOptions`]. Upstream names the
/// thinking-level field `reasoning` (not `thinkingLevel`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimpleStreamOptions {
    /// Base options inherited from upstream `StreamOptions`.
    #[serde(flatten)]
    pub stream: StreamOptions,
    /// Provider-neutral tool selection for simple requests. When omitted,
    /// adapters use provider-specific behavior (types.ts:320).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Reasoning/thinking level for simple requests (types.ts:321; upstream
    /// field name is `reasoning`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ThinkingLevel>,
    /// Ask a capable provider to return a durable handle and continue the
    /// request asynchronously (types.ts:323).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred: Option<DeferredFlag>,
    /// Custom token budgets for thinking levels (token-based providers only)
    /// (types.ts:325).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budgets: Option<ThinkingBudgets>,
}

/// Upstream `SimpleStreamOptions.deferred` union (types.ts:323):
/// `boolean | { window?: "15m" | "1h" | "24h" }`. `true` defers with the
/// provider's default window; the object form names the desired retention
/// window. Untagged so both wire shapes round-trip exactly as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeferredFlag {
    Bool(bool),
    Object {
        /// Desired retention window for the deferred response.
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<DeferredWindow>,
    },
}

/// Upstream deferred window union (types.ts:323): `"15m" | "1h" | "24h"`.
/// The wire values are not representable by any casing rule, so each variant
/// carries an explicit rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeferredWindow {
    #[serde(rename = "15m")]
    FifteenMinutes,
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "24h")]
    TwentyFourHours,
}

/// Upstream `DeferredHandle` (types.ts:462-472): a durable handle returned by
/// a provider that deferred a request, used to resume polling for the final
/// response. `data` holds the provider conversion data required to
/// reconstruct the final assistant message; `serde_json::Value` is the port's
/// `JsonValue` (same pattern as `ToolCall.arguments`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredHandle {
    pub provider: String,
    pub model_id: String,
    pub api: String,
    /// Provider token, such as a response id or batch id plus row id.
    pub id: String,
    /// Unix timestamp in milliseconds when the handle expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// Milliseconds to wait before the first status poll.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_after_ms: Option<u64>,
    /// Provider conversion data required to reconstruct the final assistant
    /// message.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_support::present_json"
    )]
    pub data: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_handle_full_round_trips() {
        let fixture = r#"{"provider":"anthropic","modelId":"claude-sonnet-4-5","api":"anthropic-messages","id":"resp_123","expiresAt":1758240000000,"pollAfterMs":5000,"data":{"state":"pending"}}"#;
        let handle: DeferredHandle = serde_json::from_str(fixture).unwrap();
        assert_eq!(handle.provider, "anthropic");
        assert_eq!(handle.model_id, "claude-sonnet-4-5");
        assert_eq!(handle.api, "anthropic-messages");
        assert_eq!(handle.id, "resp_123");
        assert_eq!(handle.expires_at, Some(1758240000000));
        assert_eq!(handle.poll_after_ms, Some(5000));
        assert_eq!(handle.data, Some(serde_json::json!({"state": "pending"})));
        assert_eq!(serde_json::to_string(&handle).unwrap(), fixture);
    }

    #[test]
    fn deferred_handle_minimal_omits_optionals_and_requires_required() {
        let fixture =
            r#"{"provider":"openai","modelId":"gpt-5","api":"openai-responses","id":"resp_abc"}"#;
        let handle: DeferredHandle = serde_json::from_str(fixture).unwrap();
        assert_eq!(handle.expires_at, None);
        assert_eq!(handle.poll_after_ms, None);
        assert_eq!(handle.data, None);
        assert_eq!(serde_json::to_string(&handle).unwrap(), fixture);

        // Missing any required field is an error.
        assert!(serde_json::from_str::<DeferredHandle>(r#"{"provider":"anthropic"}"#).is_err());
        assert!(serde_json::from_str::<DeferredHandle>(
            r#"{"provider":"anthropic","modelId":"m","api":"anthropic-messages"}"#
        )
        .is_err());
    }

    #[test]
    fn deferred_flag_round_trips_all_shapes() {
        let cases: Vec<(&str, DeferredFlag)> = vec![
            ("true", DeferredFlag::Bool(true)),
            ("false", DeferredFlag::Bool(false)),
            ("{}", DeferredFlag::Object { window: None }),
            (
                r#"{"window":"15m"}"#,
                DeferredFlag::Object {
                    window: Some(DeferredWindow::FifteenMinutes),
                },
            ),
            (
                r#"{"window":"1h"}"#,
                DeferredFlag::Object {
                    window: Some(DeferredWindow::OneHour),
                },
            ),
            (
                r#"{"window":"24h"}"#,
                DeferredFlag::Object {
                    window: Some(DeferredWindow::TwentyFourHours),
                },
            ),
        ];
        for (wire, value) in cases {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            let back: DeferredFlag = serde_json::from_str(wire).unwrap();
            assert_eq!(back, value);
        }
        // Unknown windows and non-bool/object shapes are rejected.
        assert!(serde_json::from_str::<DeferredFlag>(r#"{"window":"25h"}"#).is_err());
        assert!(serde_json::from_str::<DeferredFlag>(r#""24h""#).is_err());
    }

    #[test]
    fn provider_request_options_full_round_trips() {
        let fixture = r#"{"apiKey":"sk-openai","env":{"OPENAI_BASE_URL":"https://example.com/v1"},"headers":{"x-custom":"v","x-suppressed":null},"timeoutMs":600000,"maxRetries":2,"maxRetryDelayMs":60000}"#;
        let options: ProviderRequestOptions = serde_json::from_str(fixture).unwrap();
        assert_eq!(options.api_key, Some("sk-openai".into()));
        assert_eq!(
            options.env.as_ref().unwrap().get("OPENAI_BASE_URL"),
            Some(&"https://example.com/v1".to_string())
        );
        let headers = options.headers.as_ref().unwrap();
        assert_eq!(headers.get("x-custom"), Some(&Some("v".into())));
        // A null header value (upstream `string | null`) suppresses a default.
        assert_eq!(headers.get("x-suppressed"), Some(&None));
        assert_eq!(options.timeout_ms, Some(600000));
        assert_eq!(options.max_retries, Some(2));
        assert_eq!(options.max_retry_delay_ms, Some(60000));
        assert_eq!(serde_json::to_string(&options).unwrap(), fixture);
        assert_eq!(
            serde_json::to_string(&ProviderRequestOptions::default()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn stream_options_full_round_trips() {
        let fixture = r#"{"apiKey":"sk-openai","env":{"OPENAI_BASE_URL":"https://example.com/v1"},"headers":{"x-custom":"v"},"timeoutMs":600000,"maxRetries":2,"maxRetryDelayMs":60000,"temperature":0.7,"samplingParams":{"top_k":40},"maxTokens":4096,"transport":"websocket","cacheRetention":"long","sessionId":"sess_123","websocketConnectTimeoutMs":15000,"metadata":{"userId":"user-1"}}"#;
        let options: StreamOptions = serde_json::from_str(fixture).unwrap();
        assert_eq!(options.api_key, Some("sk-openai".into()));
        assert_eq!(options.timeout_ms, Some(600000));
        assert_eq!(options.max_retries, Some(2));
        assert_eq!(options.max_retry_delay_ms, Some(60000));
        assert_eq!(options.temperature, Some(0.7));
        assert_eq!(
            options.sampling_params.as_ref().unwrap().get("top_k"),
            Some(&serde_json::json!(40))
        );
        assert_eq!(options.max_tokens, Some(4096));
        assert_eq!(options.transport, Some(Transport::Websocket));
        assert_eq!(options.cache_retention, Some(CacheRetention::Long));
        assert_eq!(options.session_id, Some("sess_123".into()));
        assert_eq!(options.websocket_connect_timeout_ms, Some(15000));
        assert_eq!(
            options.metadata.as_ref().unwrap().get("userId"),
            Some(&serde_json::json!("user-1"))
        );
        assert_eq!(serde_json::to_string(&options).unwrap(), fixture);
        assert_eq!(
            serde_json::to_string(&StreamOptions::default()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn simple_stream_options_deferred_bool_round_trips() {
        let fixture = r#"{"temperature":0.7,"maxTokens":1024,"deferred":true}"#;
        let options: SimpleStreamOptions = serde_json::from_str(fixture).unwrap();
        assert_eq!(options.stream.temperature, Some(0.7));
        assert_eq!(options.stream.max_tokens, Some(1024));
        assert_eq!(options.deferred, Some(DeferredFlag::Bool(true)));
        assert_eq!(serde_json::to_string(&options).unwrap(), fixture);
    }

    #[test]
    fn simple_stream_options_deferred_object_round_trips() {
        let fixture = r#"{"reasoning":"high","deferred":{"window":"24h"}}"#;
        let options: SimpleStreamOptions = serde_json::from_str(fixture).unwrap();
        assert_eq!(options.reasoning, Some(ThinkingLevel::High));
        assert_eq!(
            options.deferred,
            Some(DeferredFlag::Object {
                window: Some(DeferredWindow::TwentyFourHours),
            })
        );
        assert_eq!(serde_json::to_string(&options).unwrap(), fixture);
    }

    #[test]
    fn simple_stream_options_full_round_trips() {
        let fixture = r#"{"apiKey":"sk-openai","env":{"OPENAI_BASE_URL":"https://example.com/v1"},"headers":{"x-custom":"v","x-suppressed":null},"timeoutMs":600000,"maxRetries":2,"maxRetryDelayMs":60000,"temperature":0.7,"samplingParams":{"top_k":40},"maxTokens":4096,"transport":"websocket","cacheRetention":"long","sessionId":"sess_123","websocketConnectTimeoutMs":15000,"metadata":{"userId":"user-1"},"toolChoice":"auto","reasoning":"high","deferred":{"window":"24h"},"thinkingBudgets":{"low":1024,"high":8192}}"#;
        let options: SimpleStreamOptions = serde_json::from_str(fixture).unwrap();
        assert_eq!(options.stream.api_key, Some("sk-openai".into()));
        assert_eq!(options.stream.temperature, Some(0.7));
        assert_eq!(options.stream.transport, Some(Transport::Websocket));
        assert_eq!(options.stream.cache_retention, Some(CacheRetention::Long));
        assert_eq!(options.tool_choice, Some(ToolChoice::Auto));
        assert_eq!(options.reasoning, Some(ThinkingLevel::High));
        assert_eq!(
            options.deferred,
            Some(DeferredFlag::Object {
                window: Some(DeferredWindow::TwentyFourHours),
            })
        );
        assert_eq!(
            options.thinking_budgets,
            Some(ThinkingBudgets {
                minimal: None,
                low: Some(1024),
                medium: None,
                high: Some(8192),
            })
        );
        assert_eq!(serde_json::to_string(&options).unwrap(), fixture);
    }

    #[test]
    fn simple_stream_options_defaults_to_empty_object() {
        assert_eq!(
            serde_json::to_string(&SimpleStreamOptions::default()).unwrap(),
            "{}"
        );
        let minimal: SimpleStreamOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(minimal.deferred, None);
        assert_eq!(minimal.tool_choice, None);
        assert_eq!(minimal.reasoning, None);
        assert_eq!(minimal.thinking_budgets, None);
        assert_eq!(minimal.stream, StreamOptions::default());
    }

    /// The signal is transport-only (`#[serde(skip)]`): a set token never
    /// reaches the wire, and an incoming `signal` key is ignored rather than
    /// rejected, exactly like upstream where `AbortSignal` is a runtime
    /// object no JSON payload carries.
    #[test]
    fn signal_is_transport_only_and_never_serializes() {
        let mut options = StreamOptions::default();
        assert_eq!(options.signal, None);
        options.signal = Some(tokio_util::sync::CancellationToken::new());
        assert_eq!(serde_json::to_string(&options).unwrap(), "{}");
        // Unknown `signal` keys on the wire do not reject deserialization.
        let parsed: StreamOptions = serde_json::from_str(r#"{"signal":{}}"#).unwrap();
        assert_eq!(parsed.signal, None);
        assert_eq!(parsed, StreamOptions::default());
    }
}
