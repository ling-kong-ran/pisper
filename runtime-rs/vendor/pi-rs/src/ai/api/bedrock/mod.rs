//! Amazon Bedrock ConverseStream API — full port of the stream/streamSimple
//! implementations from upstream
//! `packages/ai/src/api/bedrock-converse-stream.ts` (1344 lines): the
//! ConverseStream JSON request, the AWS event-stream response framing (see
//! [`event_stream`]), SigV4/bearer authentication (see [`signing`]), the
//! endpoint/region resolution matrix, the streaming block state machine
//! (text/thinking/toolCall keyed by wire `contentBlockIndex`), encrypted
//! (`redactedContent`) and signed reasoning, prompt-cache points, the
//! Converse stop-reason mapping, and the Bedrock failure diagnostics.
//!
//! Deviations from upstream, all structural (mirroring the sibling ports):
//! - **Auth surface**: upstream delegates to `@aws-sdk/client-bedrock-runtime`,
//!   whose default credential chain (instance role, SSO, `~/.aws/credentials`
//!   profiles) lands with ambient auth in M2d. The port resolves static
//!   credentials exactly like upstream's explicit-config branch (env keys,
//!   `AWS_BEDROCK_SKIP_AUTH` dummies, bearer token) and fails with the SDK's
//!   `Could not load credentials from any providers` message when nothing
//!   resolves. `options.profile` has no port input; a profile only shapes
//!   endpoint/region decisions (upstream lines 160-203) and suppresses env
//!   credentials, but profile-file credential/region loading is deferred to
//!   M2d — when only an ambient profile could supply a region, the port fails
//!   with the SDK's region error instead of guessing one.
//! - **Signing**: raw reqwest + `aws-sigv4` (pinned exact, per the
//!   architecture decision) replaces the SDK middleware stack; the signed
//!   header set is the same (host via URI, x-amz-date, session token,
//!   content-type, injected custom headers). SDK transport headers
//!   (`amz-sdk-invocation-id`, `amz-sdk-request`) are not reproduced. A
//!   retried request re-sends the same signature (valid within the SigV4
//!   clock-skew window; retries are immediate).
//! - **onPayload/onResponse have no port surface** (M2a options omission).
//!   Oracle assertions against the captured command input read the wire body
//!   instead (same builder decisions); the response-headers middleware's
//!   observable effect — the `x-amzn-requestid` diagnostic fallback — is
//!   captured from the raw response headers.
//! - `options.region`, `thinkingDisplay`, `interleavedThinking`,
//!   `requestMetadata`, and `bearerToken` (the BedrockOptions extension
//!   fields) have no port input: `region` rides the env chain,
//!   `thinkingDisplay`/`interleavedThinking` keep their upstream defaults
//!   (`"summarized"` / true), and the bearer path keys off `options.apiKey`
//!   (then `ProviderConfig.api_key`, per the M2c key-resolution ruling).
//!   `streamSimple` reasoning shaping is the port surface for the thinking
//!   payloads.
//! - **Framing**: the JS SDK deserializes AWS event-stream binary frames into
//!   `{ [event-type]: payload }` objects; the port implements that decoder
//!   ([`event_stream`], wire format documented there). Response frames are
//!   CRC-checked but not signed (Bedrock does not sign ConverseStream
//!   response frames). Mid-stream modeled exceptions reach upstream's catch
//!   as bare object literals (no `.name`, no `$metadata`), so the port's
//!   exception failures carry the serialized payload as the message and no
//!   error code.
//! - `sanitizeSurrogates` is a no-op (Rust `String` is UTF-8 and cannot hold
//!   unpaired surrogates; the placeholder branches it feeds are covered by
//!   the blank-text oracles). Negative usage token counts saturate at 0
//!   (u64 wire counts). Missing `contentBlockIndex` on a start frame skips
//!   the block and on a delta frame degrades to index 0, where upstream's
//!   `!` assertion would poison the block search. Unknown content block
//!   types are unrepresentable in the typed transcript model, so the
//!   "skips unknown content blocks" oracles reduce to the blank/placeholder
//!   branches.
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order — the established port-wide deviation; wire assertions are
//!   structural.
//! - `options.signal` has no port input, so the post-stream abort check and
//!   the catch block's `"aborted"` branch are unreachable — error events
//!   always carry reason `"error"`.
//! - The initial request goes through the provider-request retry seam
//!   ([`crate::ai::retry::retry_provider_request`]) with `options.maxRetries`
//!   (default 0 — the pi SDKs upstream run with `maxRetries: 0`); once event
//!   bytes flow an error is never retried.

pub mod event_stream;
pub mod signing;

use std::time::Duration;

use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::ai::api::azure_openai_responses::get_provider_env_value;
use crate::ai::api::bedrock::event_stream::{base64_decode, base64_encode, Frame, FrameDecoder};
use crate::ai::api::bedrock::signing::{
    bearer_authorization, dummy_credentials, is_reserved_header, sign_request, AwsCredentials,
};
use crate::ai::api::openai_completions::request::{
    clamp_max_tokens_to_context, make_strict_json_schema, resolve_json_schema_strict_sampling,
    set_header, thinking_budget_for_level, transform_messages, MIN_ANSWER_TOKENS,
};
#[cfg(test)]
mod callback_tests;

use crate::ai::api::openai_completions::stream::{parse_streaming_json, truncate_error_text};
use crate::ai::api::{http_client, request_signal, ApiImpl, REQUEST_WAS_ABORTED};
use crate::ai::cost::calculate_cost;
use crate::ai::retry::{retry_provider_request, ProviderError};
use crate::ai::transcript::{
    collapse_system_messages, get_current_tools, get_initial_system_message,
    get_system_message_text, without_initial_system_message, TranscriptContext,
};
use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, AssistantMessageDiagnostic, Message, StringOrBlocks,
    TextOrImageBlock, ToolResultMessage,
};
use crate::ai::types::options::{ProviderEnv, SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{CacheRetention, StopReason, ThinkingBudgets, ThinkingLevel};
use crate::ai::types::tool::Tool;
use crate::ai::types::Model;
use crate::ai::{now_ms, ProviderConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The API id stamped on every emitted message.
const API: &str = "bedrock-converse-stream";

/// Upstream `EMPTY_TEXT_PLACEHOLDER` (line 120).
const EMPTY_TEXT_PLACEHOLDER: &str = "<empty>";

/// Upstream `REDACTED_THINKING_PLACEHOLDER` (line 123) — matches the
/// placeholder the Anthropic API path uses for redacted thinking.
const REDACTED_THINKING_PLACEHOLDER: &str = "[Reasoning redacted]";

/// Upstream `BEDROCK_ERROR_PREFIXES` (lines 368-374): human-readable prefixes
/// for the modeled Bedrock SDK exception names. Downstream retry logic
/// matches patterns like `server.?error`, so the legacy prefix format is
/// preserved rather than the raw SDK exception name.
const BEDROCK_ERROR_PREFIXES: &[(&str, &str)] = &[
    ("InternalServerException", "Internal server error"),
    ("ModelStreamErrorException", "Model stream error"),
    ("ValidationException", "Validation error"),
    ("ThrottlingException", "Throttling error"),
    ("ServiceUnavailableException", "Service unavailable"),
];

/// Upstream `BEDROCK_DATA_RETENTION_DOCS_URL` (line 381).
const BEDROCK_DATA_RETENTION_DOCS_URL: &str =
    "https://docs.aws.amazon.com/bedrock/latest/userguide/data-retention.html";

/// Upstream `MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS` (line 412). Over-long
/// header values are dropped rather than truncated: a truncated request id is
/// not a request id.
const MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS: usize = 200;

/// Upstream `MAX_PROVIDER_ERROR_BODY_CHARS` (`utils/error-body.ts`).
const MAX_PROVIDER_ERROR_BODY_CHARS: usize = 4000;

// =============================================================================
// Options (bedrock-converse-stream.ts:79-111)
// =============================================================================

/// Upstream `BedrockOptions` extension fields over the base options: internal
/// because the public entry points default the extensions the way the only
/// reachable upstream flows do (`streamSimple` derives `toolChoice`,
/// `reasoning`, and `thinkingBudgets`; direct `stream` callers pass none).
/// The remaining upstream extensions (`region`, `profile`, `thinkingDisplay`,
/// `interleavedThinking`, `requestMetadata`, `bearerToken`) have no port
/// input — see the module docs.
#[derive(Debug, Clone, Default)]
struct BedrockOptions {
    /// Base options (upstream `StreamOptions` inheritance).
    stream: StreamOptions,
    /// Upstream `toolChoice`; the port surface produces `Auto`/`None` via
    /// streamSimple (`Any` and the named-tool arm have no port input).
    tool_choice: Option<BedrockToolChoice>,
    /// Upstream `reasoning` (thinking level).
    reasoning: Option<ThinkingLevel>,
    /// Upstream `thinkingBudgets` (custom per-level token budgets).
    thinking_budgets: Option<ThinkingBudgets>,
}

/// Upstream `BedrockOptions["toolChoice"]`
/// (`"auto" | "any" | "none" | { type: "tool"; name }`). `Any` and `Tool`
/// have no port input (the `SimpleStreamOptions.toolChoice` surface carries
/// auto/none only) but stay in the union so `convertToolConfig`'s mapping is
/// complete.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BedrockToolChoice {
    Auto,
    #[allow(dead_code)]
    Any,
    None,
    #[allow(dead_code)]
    Tool(String),
}

impl From<crate::ai::types::primitives::ToolChoice> for BedrockToolChoice {
    fn from(choice: crate::ai::types::primitives::ToolChoice) -> Self {
        match choice {
            crate::ai::types::primitives::ToolChoice::Auto => BedrockToolChoice::Auto,
            crate::ai::types::primitives::ToolChoice::None => BedrockToolChoice::None,
        }
    }
}

// =============================================================================
// Entry points (bedrock-converse-stream.ts:125-360, 528-577)
// =============================================================================

pub struct BedrockConverseStream;

impl ApiImpl for BedrockConverseStream {
    // Upstream wires both `options?.onPayload?.(commandInput, model)`
    // (bedrock-converse-stream.ts:280) and onResponse (lines 251-255 install
    // the deserialize-step response-headers middleware; line 293 is the
    // synthesized-metadata fallback).
    fn supports_request_callbacks(&self) -> bool {
        true
    }

    fn stream(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream line 132: collapse mid-conversation system messages (Bedrock
        // has none). Direct-`stream` callers pass no Bedrock extension options.
        let normalized = collapse_system_messages(ctx.clone());
        run_stream(
            cfg.clone(),
            model.clone(),
            normalized,
            BedrockOptions {
                stream: options.clone(),
                ..BedrockOptions::default()
            },
        )
    }

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream `streamSimple` (lines 528-577): buildBaseOptions shaping
        // (context-clamped maxTokens default over the original context) plus
        // the Claude thinking adjustments.
        let mut stream_options = options.stream.clone();
        stream_options.max_tokens = Some(clamp_max_tokens_to_context(
            model,
            ctx,
            options.stream.max_tokens.unwrap_or(model.max_tokens),
        ));
        let base = BedrockOptions {
            stream: stream_options,
            tool_choice: options.tool_choice.map(BedrockToolChoice::from),
            reasoning: options.reasoning,
            thinking_budgets: options.thinking_budgets,
        };
        let Some(reasoning) = options.reasoning else {
            // No reasoning requested: stream with reasoning disabled.
            return run_stream(
                cfg.clone(),
                model.clone(),
                collapse_system_messages(ctx.clone()),
                base,
            );
        };

        let normalized = collapse_system_messages(ctx.clone());
        if is_anthropic_claude_model(model) {
            if supports_adaptive_thinking(model) {
                return run_stream(cfg.clone(), model.clone(), normalized, base);
            }
            // Undefined means the caller did not request an output cap; let the
            // helper use the model cap (upstream lines 550-557).
            let (adjusted_max_tokens, thinking_budget) = adjust_max_tokens_for_thinking(
                base.stream.max_tokens,
                model.max_tokens,
                reasoning,
                base.thinking_budgets.as_ref(),
            );
            let max_tokens = clamp_max_tokens_to_context(model, &normalized, adjusted_max_tokens);
            let mut budgets = base.thinking_budgets.unwrap_or_default();
            let budget = thinking_budget.min(max_tokens.saturating_sub(MIN_ANSWER_TOKENS));
            let slot = u32::try_from(budget).unwrap_or(u32::MAX);
            match clamp_reasoning(reasoning) {
                ThinkingLevel::Minimal => budgets.minimal = Some(slot),
                ThinkingLevel::Low => budgets.low = Some(slot),
                ThinkingLevel::Medium => budgets.medium = Some(slot),
                _ => budgets.high = Some(slot),
            }
            return run_stream(
                cfg.clone(),
                model.clone(),
                normalized,
                BedrockOptions {
                    stream: StreamOptions {
                        max_tokens: Some(max_tokens),
                        ..base.stream
                    },
                    reasoning: Some(reasoning),
                    thinking_budgets: Some(budgets),
                    tool_choice: base.tool_choice,
                },
            );
        }

        run_stream(cfg.clone(), model.clone(), normalized, base)
    }
}

/// Upstream `adjustMaxTokensForThinking` (`api/simple-options.ts:79-95`),
/// shared with the Anthropic port's copy.
fn adjust_max_tokens_for_thinking(
    base_max_tokens: Option<u64>,
    model_max_tokens: u64,
    reasoning_level: ThinkingLevel,
    custom_budgets: Option<&ThinkingBudgets>,
) -> (u64, u64) {
    let mut thinking_budget = thinking_budget_for_level(reasoning_level, custom_budgets);
    let max_tokens = match base_max_tokens {
        None => model_max_tokens,
        Some(base) => base.saturating_add(thinking_budget).min(model_max_tokens),
    };
    if max_tokens <= thinking_budget {
        thinking_budget = thinking_budget.min(max_tokens.saturating_sub(MIN_ANSWER_TOKENS));
    }
    (max_tokens, thinking_budget)
}

/// Upstream `clampReasoning` (`api/simple-options.ts:65-67`): xhigh/max clamp
/// to the `high` budget slot.
fn clamp_reasoning(level: ThinkingLevel) -> ThinkingLevel {
    match level {
        ThinkingLevel::Xhigh | ThinkingLevel::Max => ThinkingLevel::High,
        other => other,
    }
}

// =============================================================================
// Endpoint / region / auth resolution (bedrock-converse-stream.ts:155-241)
// =============================================================================

/// The resolved client configuration (upstream `BedrockRuntimeClientConfig`
/// fields the port consumes): endpoint pinning, signing region, profile,
/// and the auth inputs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ResolvedEndpointConfig {
    /// Upstream `config.profile`: the explicit option or scoped `AWS_PROFILE`
    /// (upstream `optionsProfile`), else the ambient process-env
    /// `AWS_PROFILE`.
    pub profile: Option<String>,
    /// Upstream `config.endpoint`: pinned when the model base URL is a custom
    /// (non-standard) endpoint, or a standard endpoint with no region and no
    /// ambient profile configured.
    pub endpoint: Option<String>,
    /// Upstream `config.region`: ARN-embedded > explicit option/env >
    /// endpoint-derived > us-east-1 default; `None` only when an ambient
    /// profile would resolve it (profile-file loading is SDK behavior the
    /// port does not reproduce — a profile-only configuration then fails at
    /// the signing-input check).
    pub region: Option<String>,
    /// Upstream `AWS_BEDROCK_SKIP_AUTH=1`: sign with dummy credentials.
    pub skip_auth: bool,
    /// Upstream bearer token (`config.token`): `options.apiKey` (with the
    /// provider-config key as the port's fallback) or
    /// `AWS_BEARER_TOKEN_BEDROCK`.
    pub bearer_token: Option<String>,
    /// Upstream `useBearerToken`: bearer auth replaces SigV4 entirely.
    pub use_bearer: bool,
    /// Upstream `config.credentials`: env static keys (suppressed when a
    /// profile is configured, like upstream's `!optionsProfile` guard) or the
    /// skip-auth dummies.
    pub credentials: Option<AwsCredentials>,
}

/// Resolves the client configuration, upstream lines 155-241 (the Node
/// branch — the port always runs there). `ambient_profile` mirrors
/// `Boolean(getProviderEnvValue("AWS_PROFILE"))` (process env only; scoped
/// env does not count) and is injected for testability; `config_api_key` is
/// the provider-config key (upstream `options.apiKey`'s port fallback).
pub(crate) fn resolve_endpoint_config(
    model: &Model,
    env: Option<&ProviderEnv>,
    ambient_profile: bool,
    config_api_key: Option<&str>,
) -> ResolvedEndpointConfig {
    // Upstream `optionsProfile` (line 160): `options.profile` (no port
    // surface) or the SCOPED `AWS_PROFILE` only — an ambient process-env
    // profile does not count. It gates the static-key suppression below.
    let options_profile = env
        .and_then(|env| env.get("AWS_PROFILE"))
        .filter(|value| !value.is_empty())
        .cloned();
    // Upstream `config.profile` (line 163): `optionsProfile ||`
    // `getProviderEnvValue("AWS_PROFILE", options.env)` — the scoped value
    // when present, else the ambient process env.
    let profile = options_profile
        .clone()
        .or_else(|| get_provider_env_value("AWS_PROFILE", env));
    let configured_region = get_provider_env_value("AWS_REGION", env)
        .or_else(|| get_provider_env_value("AWS_DEFAULT_REGION", env));
    let use_explicit_endpoint = should_use_explicit_bedrock_endpoint(
        &model.base_url,
        configured_region.as_deref(),
        ambient_profile,
    );
    let endpoint = use_explicit_endpoint.then(|| model.base_url.clone());
    let endpoint_region = get_standard_bedrock_endpoint_region(&model.base_url);

    // Region resolution: ARN-embedded > explicit option > env vars > SDK
    // default chain (upstream lines 190-203).
    let region = if let Some(arn_region) = extract_arn_region(&model.id) {
        Some(arn_region)
    } else if let Some(configured) = configured_region {
        Some(configured)
    } else if use_explicit_endpoint && endpoint_region.is_some() {
        endpoint_region
    } else if !ambient_profile {
        Some("us-east-1".to_string())
    } else {
        None
    };

    let skip_auth = get_provider_env_value("AWS_BEDROCK_SKIP_AUTH", env).as_deref() == Some("1");
    let bearer_token = config_api_key
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .or_else(|| get_provider_env_value("AWS_BEARER_TOKEN_BEDROCK", env));
    let use_bearer = bearer_token.is_some() && !skip_auth;
    let credentials = if skip_auth {
        Some(dummy_credentials())
    } else {
        // Upstream line 215: `!skipAuth && credentials && !optionsProfile` —
        // an ambient-only AWS_PROFILE keeps the static keys (the SDK chain
        // resolves the profile but the ambient keys still win there; see the
        // bedrock-credentials.test.ts oracle).
        configured_bedrock_credentials(env).filter(|_| options_profile.is_none())
    };

    ResolvedEndpointConfig {
        profile,
        endpoint,
        region,
        skip_auth,
        bearer_token,
        use_bearer,
        credentials,
    }
}

/// Upstream `getConfiguredBedrockCredentials` (lines 1184-1196): both keys
/// required, optional session token.
fn configured_bedrock_credentials(env: Option<&ProviderEnv>) -> Option<AwsCredentials> {
    let access_key_id = get_provider_env_value("AWS_ACCESS_KEY_ID", env)?;
    let secret_access_key = get_provider_env_value("AWS_SECRET_ACCESS_KEY", env)?;
    let session_token = get_provider_env_value("AWS_SESSION_TOKEN", env);
    Some(AwsCredentials {
        access_key_id,
        secret_access_key,
        session_token,
    })
}

/// Upstream `getStandardBedrockEndpointRegion` (lines 1198-1210): the region
/// of a standard `bedrock-runtime[-fips].{region}.amazonaws.com[.cn]` host.
pub(crate) fn get_standard_bedrock_endpoint_region(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let hostname = url.host_str()?.to_ascii_lowercase();
    let tail = hostname
        .strip_prefix("bedrock-runtime.")
        .or_else(|| hostname.strip_prefix("bedrock-runtime-fips."))?;
    let region = tail
        .strip_suffix(".amazonaws.com.cn")
        .or_else(|| tail.strip_suffix(".amazonaws.com"))?;
    if region.is_empty()
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }
    Some(region.to_string())
}

/// Upstream `shouldUseExplicitBedrockEndpoint` (lines 1212-1223): custom
/// endpoints always pass through; standard endpoints are pinned only when no
/// region or ambient profile is configured.
pub(crate) fn should_use_explicit_bedrock_endpoint(
    base_url: &str,
    configured_region: Option<&str>,
    ambient_profile: bool,
) -> bool {
    if get_standard_bedrock_endpoint_region(base_url).is_none() {
        return true;
    }
    configured_region.is_none() && !ambient_profile
}

/// Upstream line 194:
/// `model.id.match(/^arn:aws(?:-[a-z0-9-]+)?:bedrock:([a-z0-9-]+):/)[1]`:
/// the optional partition suffix accepts any `arn:aws-<partition>` (us-gov,
/// cn, iso, ...), not just `-us-gov`.
fn extract_arn_region(model_id: &str) -> Option<String> {
    let rest = model_id.strip_prefix("arn:aws")?;
    let rest = match rest.strip_prefix('-') {
        // `-<partition>:...`: skip to the colon, keeping it (upstream's
        // `(?:-[a-z0-9-]+)?` requires a non-empty lowercase/digit/dash
        // partition).
        Some(partition) => {
            let index = partition.find(':')?;
            if index == 0
                || !partition[..index]
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return None;
            }
            &partition[index..]
        }
        None => rest,
    };
    let rest = rest.strip_prefix(":bedrock:")?;
    let region = rest.split(':').next()?;
    if region.is_empty()
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }
    Some(region.to_string())
}

/// Upstream `isGovCloudBedrockTarget` (lines 1225-1233).
fn is_gov_cloud_bedrock_target(model: &Model, env: Option<&ProviderEnv>) -> bool {
    let configured_region = get_provider_env_value("AWS_REGION", env)
        .or_else(|| get_provider_env_value("AWS_DEFAULT_REGION", env));
    if configured_region
        .as_deref()
        .is_some_and(|region| region.to_ascii_lowercase().starts_with("us-gov-"))
    {
        return true;
    }
    let model_id = model.id.to_ascii_lowercase();
    model_id.starts_with("us-gov.") || model_id.starts_with("arn:aws-us-gov:")
}

/// The ConverseStream request URL: `{endpoint}/model/{modelId+}/stream`. The
/// greedy path label keeps `/` separators (inference profile ARNs) and
/// extended-encodes each segment, matching the JS SDK's `resolvedPath`.
pub(crate) fn build_stream_url(
    resolved: &ResolvedEndpointConfig,
    model_id: &str,
) -> Result<String, String> {
    let region = resolved.region.as_deref().ok_or_else(|| {
        "Unable to determine a region from the configured sources. Please configure a region or profile."
            .to_string()
    })?;
    let base = match &resolved.endpoint {
        Some(endpoint) => endpoint.trim_end_matches('/').to_string(),
        // SDK endpoint resolver: cn-* regions live under the .cn domain.
        None if region.starts_with("cn-") => {
            format!("https://bedrock-runtime.{region}.amazonaws.com.cn")
        }
        None => format!("https://bedrock-runtime.{region}.amazonaws.com"),
    };
    let encoded: Vec<String> = model_id
        .split('/')
        .map(extended_encode_uri_component)
        .collect();
    Ok(format!("{base}/model/{}/stream", encoded.join("/")))
}

/// JS `extendedEncodeURIComponent`: everything outside the unreserved set
/// `A-Za-z0-9-_.~` is percent-encoded.
fn extended_encode_uri_component(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for &byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// =============================================================================
// Model predicates (bedrock-converse-stream.ts:755-885)
// =============================================================================

/// Upstream `getModelMatchCandidates` (lines 755-761): both the model id and
/// name, lowercased, plus dash-normalized variants.
fn get_model_match_candidates(model: &Model) -> Vec<String> {
    let mut candidates = Vec::with_capacity(4);
    for value in [model.id.as_str(), model.name.as_str()] {
        let lower = value.to_ascii_lowercase();
        let normalized = lower
            .chars()
            .map(|character| {
                if character.is_whitespace() || matches!(character, '_' | '.' | ':') {
                    '-'
                } else {
                    character
                }
            })
            .collect::<String>();
        candidates.push(lower);
        candidates.push(normalized);
    }
    candidates
}

/// Upstream `supportsAdaptiveThinking` (lines 763-775): Opus 4.6+, Sonnet
/// 4.6+ and the Claude 5 line, checked on id and name.
fn supports_adaptive_thinking(model: &Model) -> bool {
    get_model_match_candidates(model).iter().any(|candidate| {
        candidate.contains("opus-4-6")
            || candidate.contains("opus-4-7")
            || candidate.contains("opus-4-8")
            || candidate.contains("opus-5")
            || candidate.contains("sonnet-4-6")
            || candidate.contains("sonnet-5")
            || candidate.contains("fable-5")
    })
}

/// Upstream `supportsNativeXhighEffort` (lines 777-787).
fn supports_native_xhigh_effort(model: &Model) -> bool {
    get_model_match_candidates(model).iter().any(|candidate| {
        candidate.contains("opus-4-7")
            || candidate.contains("opus-4-8")
            || candidate.contains("opus-5")
            || candidate.contains("sonnet-5")
            || candidate.contains("fable-5")
    })
}

/// Upstream `THINKING_BINDING_CONTROLS_BETA` (v1.0.0).
const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";

/// Upstream `supportsThinkingBlockBinding` (v1.0.0): check if the model
/// accepts `thinking.block_binding`. Opus 4.6 and Sonnet 4.6 reject it with
/// "thinking.adaptive.block_binding: Extra inputs are not permitted".
fn supports_thinking_block_binding(model: &Model) -> bool {
    get_model_match_candidates(model).iter().any(|candidate| {
        candidate.contains("opus-4-7")
            || candidate.contains("opus-4-8")
            || candidate.contains("opus-5")
            || candidate.contains("sonnet-5")
            || candidate.contains("fable-5")
    })
}

/// Upstream `mapThinkingLevelToEffort` (lines 789-809): native xhigh for the
/// newest line, then the model's `thinkingLevelMap`, then the fixed fallback.
fn map_thinking_level_to_effort(model: &Model, level: Option<ThinkingLevel>) -> String {
    if level == Some(ThinkingLevel::Xhigh) && supports_native_xhigh_effort(model) {
        return "xhigh".to_string();
    }
    let mapped = level.and_then(|level| {
        let key = match level {
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::Xhigh => "xhigh",
            ThinkingLevel::Max => "max",
        };
        model
            .thinking_level_map
            .as_ref()
            .and_then(|map| map.get(key))
            .and_then(|value| value.as_deref())
            .map(str::to_string)
    });
    if let Some(mapped) = mapped {
        return mapped;
    }
    match level {
        Some(ThinkingLevel::Minimal) | Some(ThinkingLevel::Low) => "low",
        Some(ThinkingLevel::Medium) => "medium",
        _ => "high",
    }
    .to_string()
}

/// Upstream `resolveCacheRetention` (lines 815-823): explicit option, then
/// `PI_CACHE_RETENTION=long`, then `"short"`.
fn resolve_cache_retention(
    cache_retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> CacheRetention {
    if let Some(cache_retention) = cache_retention {
        return cache_retention;
    }
    if get_provider_env_value("PI_CACHE_RETENTION", env).as_deref() == Some("long") {
        return CacheRetention::Long;
    }
    CacheRetention::Short
}

/// Upstream `isAnthropicClaudeModel` (lines 830-840), checked on id and name.
fn is_anthropic_claude_model(model: &Model) -> bool {
    let id = model.id.to_ascii_lowercase();
    let name = model.name.to_ascii_lowercase();
    id.contains("anthropic.claude")
        || id.contains("anthropic/claude")
        || name.contains("anthropic.claude")
        || name.contains("anthropic/claude")
        || name.contains("claude")
}

/// Upstream `supportsPromptCaching` (lines 854-873): Claude 3.5 Haiku, 3.7
/// Sonnet, 4.x and 5.x models; non-Claude ids need `AWS_BEDROCK_FORCE_CACHE=1`.
fn supports_prompt_caching(model: &Model, env: Option<&ProviderEnv>) -> bool {
    let candidates = get_model_match_candidates(model);
    let has_claude_ref = candidates
        .iter()
        .any(|candidate| candidate.contains("claude"));
    if !has_claude_ref {
        // Application inference profiles don't contain the model name in the
        // ARN; allow users to force cache points via the environment.
        return get_provider_env_value("AWS_BEDROCK_FORCE_CACHE", env).as_deref() == Some("1");
    }
    if candidates.iter().any(|candidate| {
        candidate.contains("fable-5")
            || candidate.contains("opus-5")
            || candidate.contains("sonnet-5")
    }) {
        return true;
    }
    if candidates.iter().any(|candidate| candidate.contains("-4-")) {
        return true;
    }
    if candidates
        .iter()
        .any(|candidate| candidate.contains("claude-3-7-sonnet"))
    {
        return true;
    }
    candidates
        .iter()
        .any(|candidate| candidate.contains("claude-3-5-haiku"))
}

/// Upstream `supportsThinkingSignature` (lines 883-885): only Anthropic
/// Claude models accept `reasoningContent.reasoningText.signature`.
fn supports_thinking_signature(model: &Model) -> bool {
    is_anthropic_claude_model(model)
}

// =============================================================================
// Request payload (bedrock-converse-stream.ts:264-279 + builders)
// =============================================================================

/// Assembles the ConverseStream request body (upstream `commandInput`).
/// `requestMetadata` has no port input and is omitted.
fn build_payload(
    model: &Model,
    ctx: &TranscriptContext,
    options: &BedrockOptions,
) -> Result<Value, BedrockFailure> {
    let env = options.stream.env.as_ref();
    let cache_retention = resolve_cache_retention(options.stream.cache_retention, env);
    let inference_max_tokens = options
        .stream
        .max_tokens
        .or_else(|| is_anthropic_claude_model(model).then_some(model.max_tokens));
    let initial_system_prompt =
        get_initial_system_message(ctx.messages()).map(get_system_message_text);
    let supports_strict_mode = model
        .bedrock_compat()
        .map_err(|error| BedrockFailure::plain(error.to_string()))?
        .supports_strict_mode
        .unwrap_or(false);

    let mut payload = Map::new();
    payload.insert("modelId".to_string(), json!(model.id));
    payload.insert(
        "messages".to_string(),
        Value::Array(convert_messages(ctx, model, cache_retention, env)?),
    );
    if let Some(system) = build_system_prompt(
        initial_system_prompt.as_deref(),
        model,
        cache_retention,
        env,
    ) {
        payload.insert("system".to_string(), Value::Array(system));
    }
    let mut inference_config = Map::new();
    if let Some(max_tokens) = inference_max_tokens {
        inference_config.insert("maxTokens".to_string(), json!(max_tokens));
    }
    if let Some(temperature) = options.stream.temperature {
        inference_config.insert("temperature".to_string(), json!(temperature));
    }
    payload.insert(
        "inferenceConfig".to_string(),
        Value::Object(inference_config),
    );
    if let Some(tool_config) = convert_tool_config(
        &get_current_tools(ctx.messages()),
        options.tool_choice.as_ref(),
        supports_strict_mode,
    )? {
        payload.insert("toolConfig".to_string(), tool_config);
    }
    if let Some(additional) = build_additional_model_request_fields(
        model,
        options.reasoning,
        options.thinking_budgets.as_ref(),
        env,
    ) {
        payload.insert("additionalModelRequestFields".to_string(), additional);
    }
    Ok(Value::Object(payload))
}

/// Upstream `buildSystemPrompt` (lines 887-905): the prompt text
/// (`sanitizeSurrogates` is a no-op in Rust) plus a cache point for supported
/// Claude models when caching is enabled.
fn build_system_prompt(
    system_prompt: Option<&str>,
    model: &Model,
    cache_retention: CacheRetention,
    env: Option<&ProviderEnv>,
) -> Option<Vec<Value>> {
    let system_prompt = system_prompt?;
    let mut blocks = vec![json!({ "text": system_prompt })];
    if cache_retention != CacheRetention::None && supports_prompt_caching(model, env) {
        blocks.push(cache_point(cache_retention));
    }
    Some(blocks)
}

/// Upstream cache point literal: `{ cachePoint: { type: "default", ttl? } }`
/// where the TTL is `1h` for the long retention (`CacheTTL.ONE_HOUR`).
fn cache_point(cache_retention: CacheRetention) -> Value {
    let mut point = Map::new();
    point.insert("type".to_string(), json!("default"));
    if cache_retention == CacheRetention::Long {
        point.insert("ttl".to_string(), json!("1h"));
    }
    json!({ "cachePoint": Value::Object(point) })
}

/// Upstream `normalizeToolCallId` (lines 907-910): non-`[a-zA-Z0-9_-]`
/// characters collapse to `_`, then a 64-character cap.
fn normalize_tool_call_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    sanitized.chars().take(64).collect()
}

/// Upstream `createNonBlankTextBlock` (lines 912-915): `None` for
/// whitespace-only text.
fn create_non_blank_text_block(text: &str) -> Option<Value> {
    (!text.trim().is_empty()).then(|| json!({ "text": text }))
}

/// Upstream `createRequiredTextBlock` (lines 917-919).
fn create_required_text_block(text: &str) -> Value {
    create_non_blank_text_block(text).unwrap_or_else(|| json!({ "text": EMPTY_TEXT_PLACEHOLDER }))
}

/// Upstream `sanitizeBedrockDocument` (lines 921-933): strips empty-string
/// property keys recursively; arrays and scalars pass through.
fn sanitize_bedrock_document(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sanitize_bedrock_document).collect()),
        Value::Object(entries) => entries
            .iter()
            .filter(|(key, _)| !key.is_empty())
            .map(|(key, nested)| (key.clone(), sanitize_bedrock_document(nested)))
            .collect::<Map<String, Value>>()
            .into(),
        other => other.clone(),
    }
}

/// Upstream `createImageBlock` (lines 1285-1306): mime type mapped to the
/// Bedrock image format, base64 data passed through. The SDK transmits
/// `source.bytes` as base64, which is the port's stored form already.
fn create_image_block(mime_type: &str, data: &str) -> Result<Value, BedrockFailure> {
    let format = match mime_type {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        other => {
            return Err(BedrockFailure::plain(format!(
                "Unknown image type: {other}"
            )));
        }
    };
    Ok(json!({ "image": { "source": { "bytes": data }, "format": format } }))
}

/// Upstream `convertToolResultContent` (lines 935-947): text/image blocks
/// with blank text dropped, `<empty>` placeholder when everything drops.
fn convert_tool_result_content(content: &[TextOrImageBlock]) -> Result<Vec<Value>, BedrockFailure> {
    let mut result = Vec::new();
    for block in content {
        match block {
            TextOrImageBlock::Image(image) => {
                result.push(create_image_block(&image.mime_type, &image.data)?);
            }
            TextOrImageBlock::Text(text) => {
                if let Some(text_block) = create_non_blank_text_block(&text.text) {
                    result.push(text_block);
                }
            }
        }
    }
    if result.is_empty() {
        result.push(json!({ "text": EMPTY_TEXT_PLACEHOLDER }));
    }
    Ok(result)
}

/// Upstream `convertMessages` (lines 949-1119): user text/image blocks,
/// assistant text/toolUse/reasoning replay, consecutive tool results merged
/// into one user message, and the trailing cache point. Image format errors
/// propagate (upstream `createImageBlock` throws into the catch).
fn convert_messages(
    ctx: &TranscriptContext,
    model: &Model,
    cache_retention: CacheRetention,
    env: Option<&ProviderEnv>,
) -> Result<Vec<Value>, BedrockFailure> {
    let mut result: Vec<Value> = Vec::new();
    let normalizer = |id: &str, _source: &AssistantMessage| normalize_tool_call_id(id);
    let messages = without_initial_system_message(ctx.messages().to_vec());
    let transformed = transform_messages(model, &messages, &normalizer);

    let mut index = 0;
    while index < transformed.len() {
        let message = &transformed[index];
        match message {
            Message::User(user) => {
                let mut content = Vec::new();
                match &user.content {
                    StringOrBlocks::Text(text) => {
                        content.push(create_required_text_block(text));
                    }
                    StringOrBlocks::Blocks(blocks) => {
                        for block in blocks {
                            match block {
                                TextOrImageBlock::Text(text) => {
                                    if let Some(text_block) =
                                        create_non_blank_text_block(&text.text)
                                    {
                                        content.push(text_block);
                                    }
                                }
                                TextOrImageBlock::Image(image) => {
                                    content
                                        .push(create_image_block(&image.mime_type, &image.data)?);
                                }
                            }
                        }
                        if content.is_empty() {
                            content.push(json!({ "text": EMPTY_TEXT_PLACEHOLDER }));
                        }
                    }
                }
                result.push(json!({ "role": "user", "content": content }));
            }
            Message::Assistant(assistant) => {
                // Skip assistant messages with empty content (e.g. from aborted
                // requests); Bedrock rejects empty content arrays.
                if assistant.content.is_empty() {
                    index += 1;
                    continue;
                }
                let mut content_blocks = Vec::new();
                for block in &assistant.content {
                    match block {
                        AssistantBlock::Text(text) => {
                            if let Some(text_block) = create_non_blank_text_block(&text.text) {
                                content_blocks.push(text_block);
                            }
                        }
                        AssistantBlock::ToolCall(call) => {
                            content_blocks.push(json!({
                                "toolUse": {
                                    "toolUseId": call.id,
                                    "name": call.name,
                                    "input": sanitize_bedrock_document(&call.arguments),
                                }
                            }));
                        }
                        AssistantBlock::Thinking(thinking) => {
                            append_assistant_thinking_block(
                                &mut content_blocks,
                                model,
                                thinking.thinking.as_str(),
                                thinking.thinking_signature.as_deref(),
                                thinking.redacted == Some(true),
                            );
                        }
                    }
                }
                // Skip if all content blocks were filtered out.
                if content_blocks.is_empty() {
                    index += 1;
                    continue;
                }
                result.push(json!({ "role": "assistant", "content": content_blocks }));
            }
            Message::ToolResult(result_message) => {
                // Consecutive toolResult messages merge into a single user
                // message; Bedrock requires all tool results in one message.
                let mut tool_results = vec![tool_result_block(result_message)?];
                let mut lookahead = index + 1;
                while let Some(Message::ToolResult(next)) = transformed.get(lookahead) {
                    tool_results.push(tool_result_block(next)?);
                    lookahead += 1;
                }
                index = lookahead - 1;
                result.push(json!({ "role": "user", "content": tool_results }));
            }
            Message::System(_) => {}
        }
        index += 1;
    }

    // Trailing cache point on the last user message (lines 1106-1116).
    if cache_retention != CacheRetention::None && supports_prompt_caching(model, env) {
        if let Some(last) = result.last_mut() {
            if last.get("role") == Some(&json!("user")) {
                if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                    content.push(cache_point(cache_retention));
                }
            }
        }
    }

    Ok(result)
}

/// Upstream lines 1072-1091: one `{ toolResult: ... }` content block.
fn tool_result_block(message: &ToolResultMessage) -> Result<Value, BedrockFailure> {
    Ok(json!({
        "toolResult": {
            "toolUseId": message.tool_call_id,
            "content": convert_tool_result_content(&message.content)?,
            "status": if message.is_error { "error" } else { "success" },
        }
    }))
}

/// Upstream lines 1011-1051: the assistant thinking replay block. Redacted
/// payloads replay as `reasoningContent.redactedContent`; signed thinking
/// replays as `reasoningContent.reasoningText` (signature only on Claude
/// models, and only when a signature exists — otherwise plain text).
fn append_assistant_thinking_block(
    content_blocks: &mut Vec<Value>,
    model: &Model,
    thinking: &str,
    thinking_signature: Option<&str>,
    redacted: bool,
) {
    if redacted {
        // Encrypted reasoning is opaque: replay the stored payload as the
        // `redactedContent` member instead of lowering it to reasoning text.
        // A hand-edited session can hold a signature that is not base64;
        // drop that block instead of failing the whole request.
        let decodes = thinking_signature
            .and_then(|signature| base64_decode(signature).ok())
            .is_some_and(|bytes| !bytes.is_empty());
        if decodes {
            content_blocks.push(json!({
                "reasoningContent": { "redactedContent": thinking_signature }
            }));
        }
        return;
    }
    // Skip empty thinking blocks.
    if thinking.trim().is_empty() {
        return;
    }
    if supports_thinking_signature(model) {
        // Signatures arrive after thinking deltas. If a partial or externally
        // persisted message lacks a signature, Bedrock rejects the replayed
        // reasoning block. Fall back to plain text, matching Anthropic.
        if thinking_signature.is_none_or(|signature| signature.trim().is_empty()) {
            content_blocks.push(json!({ "text": thinking }));
        } else {
            content_blocks.push(json!({
                "reasoningContent": {
                    "reasoningText": {
                        "text": thinking,
                        "signature": thinking_signature,
                    }
                }
            }));
        }
    } else {
        content_blocks.push(json!({
            "reasoningContent": { "reasoningText": { "text": thinking } }
        }));
    }
}

/// Upstream `convertToolConfig` (lines 1121-1156).
fn convert_tool_config(
    tools: &[Tool],
    tool_choice: Option<&BedrockToolChoice>,
    supports_strict_mode: bool,
) -> Result<Option<Value>, BedrockFailure> {
    if tools.is_empty() || tool_choice == Some(&BedrockToolChoice::None) {
        return Ok(None);
    }
    let mut bedrock_tools = Vec::with_capacity(tools.len());
    for tool in tools {
        let strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode, None)
            .map_err(BedrockFailure::plain)?;
        let mut spec = json!({
            "name": tool.name,
            "description": tool.description,
            "inputSchema": { "json": get_json_schema_tool_parameters(tool, strict) },
        });
        if strict == Some(true) {
            spec["strict"] = json!(true);
        }
        bedrock_tools.push(json!({ "toolSpec": spec }));
    }

    let mut config = json!({ "tools": bedrock_tools });
    let choice = match tool_choice {
        Some(BedrockToolChoice::Auto) => Some(json!({ "auto": {} })),
        Some(BedrockToolChoice::Any) => Some(json!({ "any": {} })),
        Some(BedrockToolChoice::Tool(name)) => Some(json!({ "tool": { "name": name } })),
        _ => None,
    };
    if let Some(choice) = choice {
        config["toolChoice"] = choice;
    }
    Ok(Some(config))
}

/// Upstream `getJsonSchemaToolParameters` (`api/constrained-sampling.ts`):
/// strict conversion only when strict sampling resolves to `Some(true)`. The
/// conversion was already validated by [`resolve_json_schema_strict_sampling`],
/// so a failure here is unreachable and falls back to the verbatim schema.
fn get_json_schema_tool_parameters(tool: &Tool, strict: Option<bool>) -> Value {
    if strict == Some(true) {
        make_strict_json_schema(&tool.parameters, None).unwrap_or_else(|_| tool.parameters.clone())
    } else {
        tool.parameters.clone()
    }
}

/// Upstream `buildAdditionalModelRequestFields` (lines 1235-1283): the Claude
/// thinking controls under `additionalModelRequestFields`.
fn build_additional_model_request_fields(
    model: &Model,
    reasoning: Option<ThinkingLevel>,
    thinking_budgets: Option<&ThinkingBudgets>,
    env: Option<&ProviderEnv>,
) -> Option<Value> {
    if reasoning.is_none() || !model.reasoning {
        return None;
    }
    if !is_anthropic_claude_model(model) {
        return None;
    }

    // GovCloud Bedrock currently rejects the Claude thinking.display field.
    let is_gov_cloud = is_gov_cloud_bedrock_target(model, env);
    let display = if is_gov_cloud {
        None
    } else {
        Some("summarized")
    };
    // Replayed signed thinking blocks are bound to the system prompt and
    // tools they were created with (v1.0.0). Bedrock 400s on replay after
    // either changes unless stale blocks are dropped, matching the Anthropic
    // provider. Skipped on GovCloud like display.
    let use_block_binding = !is_gov_cloud && supports_thinking_block_binding(model);
    let mut result = Map::new();
    if supports_adaptive_thinking(model) {
        let mut thinking = Map::new();
        thinking.insert("type".to_string(), json!("adaptive"));
        if let Some(display) = display {
            thinking.insert("display".to_string(), json!(display));
        }
        if use_block_binding {
            thinking.insert(
                "block_binding".to_string(),
                json!({ "prefix_mismatch_behavior": "drop_block" }),
            );
        }
        result.insert("thinking".to_string(), Value::Object(thinking));
        result.insert(
            "output_config".to_string(),
            json!({ "effort": map_thinking_level_to_effort(model, reasoning) }),
        );
        if use_block_binding {
            result.insert(
                "anthropic_beta".to_string(),
                json!([THINKING_BINDING_CONTROLS_BETA]),
            );
        }
    } else {
        let default_budgets: [(ThinkingLevel, u32); 6] = [
            (ThinkingLevel::Minimal, 1024),
            (ThinkingLevel::Low, 2048),
            (ThinkingLevel::Medium, 8192),
            (ThinkingLevel::High, 16384),
            (ThinkingLevel::Xhigh, 16384), // Budget-based Claude clamps extended levels to high
            (ThinkingLevel::Max, 16384),
        ];
        // Custom budgets only cover token-based levels through high.
        let level = match reasoning {
            Some(ThinkingLevel::Xhigh) | Some(ThinkingLevel::Max) => ThinkingLevel::High,
            other => other.unwrap_or(ThinkingLevel::High),
        };
        let budget = thinking_budgets
            .and_then(|budgets| match level {
                ThinkingLevel::Minimal => budgets.minimal,
                ThinkingLevel::Low => budgets.low,
                ThinkingLevel::Medium => budgets.medium,
                _ => budgets.high,
            })
            .or_else(|| {
                reasoning.and_then(|reasoning| {
                    default_budgets
                        .iter()
                        .find(|(key, _)| key == &reasoning)
                        .map(|(_, budget)| *budget)
                })
            });
        let mut thinking = Map::new();
        thinking.insert("type".to_string(), json!("enabled"));
        thinking.insert("budget_tokens".to_string(), json!(budget));
        if let Some(display) = display {
            thinking.insert("display".to_string(), json!(display));
        }
        result.insert("thinking".to_string(), Value::Object(thinking));
        // interleavedThinking defaults to true upstream.
        result.insert(
            "anthropic_beta".to_string(),
            json!(["interleaved-thinking-2025-05-14"]),
        );
    }
    Some(Value::Object(result))
}

// =============================================================================
// Failures, formatting, diagnostics (bedrock-converse-stream.ts:368-455)
// =============================================================================

/// The port's stand-in for the values upstream `catch` receives: SDK service
/// exceptions (HTTP failures), mid-stream exception/error frames, and plain
/// errors. `name` is upstream `error.name`; `service_exception` is upstream
/// `instanceof BedrockRuntimeServiceException`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BedrockFailure {
    name: Option<String>,
    service_exception: bool,
    message: String,
    status: Option<u16>,
    body: Option<String>,
    /// Upstream `$metadata.requestId` (HTTP response header derived).
    metadata_request_id: Option<String>,
}

impl BedrockFailure {
    /// A plain `Error` throw: message only.
    fn plain(message: impl Into<String>) -> Self {
        BedrockFailure {
            name: None,
            service_exception: false,
            message: message.into(),
            status: None,
            body: None,
            metadata_request_id: None,
        }
    }
}

/// Upstream `formatBedrockError` (lines 390-407): surface the raw HTTP body
/// (with status) when the SDK did not fold it into the message — this is
/// what stops a gateway 403 from collapsing to `Unknown: UnknownError` — and
/// map service exception names to the human-readable prefixes.
fn format_bedrock_error(failure: &BedrockFailure) -> String {
    let body = failure
        .body
        .as_deref()
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .map(|body| truncate_error_text(body, MAX_PROVIDER_ERROR_BODY_CHARS));
    let message_carries_body = body
        .as_ref()
        .is_none_or(|body| failure.message.contains(body.as_str()));
    let core = match (!message_carries_body, failure.status, body) {
        (true, Some(status), Some(body)) => format!("{status}: {body}"),
        _ => failure.message.clone(),
    };
    let data_retention_hint = if core.to_ascii_lowercase().contains("data retention mode") {
        format!(" See {BEDROCK_DATA_RETENTION_DOCS_URL} for supported data retention modes.")
    } else {
        String::new()
    };
    if failure.service_exception {
        let name = failure.name.as_deref().unwrap_or("Unknown");
        let prefix = BEDROCK_ERROR_PREFIXES
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, prefix)| (*prefix).to_string())
            .unwrap_or_else(|| name.to_string());
        format!("{prefix}: {core}{data_retention_hint}")
    } else {
        format!("{core}{data_retention_hint}")
    }
}

/// Upstream `normalizeDiagnosticValue` (lines 414-419): trimmed, non-empty,
/// at most 200 characters.
fn normalize_diagnostic_value(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.len() > MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS {
        return None;
    }
    Some(value.to_string())
}

/// Upstream `extractBedrockErrorCode` (lines 426-429): the SDK puts the
/// modeled code on `error.name`; modeled Bedrock errors all end in
/// `Exception`, unlike transport names such as `TimeoutError`.
fn extract_bedrock_error_code(failure: &BedrockFailure) -> Option<String> {
    let name = failure.name.as_deref()?;
    if !name.ends_with("Exception") {
        return None;
    }
    normalize_diagnostic_value(Some(name))
}

/// Upstream `appendBedrockFailureDiagnostic` (lines 436-455): structured
/// metadata alongside `errorMessage` (which stays byte-identical so
/// `isRetryableAssistantError` keeps matching). Unknown fields are omitted,
/// never guessed.
fn append_bedrock_failure_diagnostic(
    output: &mut AssistantMessage,
    failure: &BedrockFailure,
    fallback_request_id: Option<&str>,
) {
    let mut details = Map::new();
    if let Some(status) = failure.status {
        details.insert("status".to_string(), json!(status));
    }
    if let Some(error_code) = extract_bedrock_error_code(failure) {
        details.insert("errorCode".to_string(), json!(error_code));
    }
    let request_id = normalize_diagnostic_value(failure.metadata_request_id.as_deref())
        .or_else(|| normalize_diagnostic_value(fallback_request_id));
    if let Some(request_id) = request_id {
        details.insert("requestId".to_string(), json!(request_id));
    }
    if details.is_empty() {
        return;
    }
    output
        .diagnostics
        .get_or_insert_with(Vec::new)
        .push(AssistantMessageDiagnostic {
            r#type: "bedrock_response_failure".to_string(),
            timestamp: now_ms(),
            error: None,
            details: Some(Value::Object(details)),
        });
}

/// Builds the failure for a non-2xx response — the value the JS SDK's
/// `handleError` path throws. The modeled code comes from
/// `x-amzn-errortype` (part before the first `:`) or the JSON body's
/// `__type` (part after the last `#`); without either, the SDK's `Unknown`
/// placeholder is used and the raw body is surfaced through the
/// `formatBedrockError` body path.
fn http_failure(
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body_text: &str,
) -> BedrockFailure {
    let trimmed = body_text.trim();
    let parsed: Option<Value> = serde_json::from_str(trimmed).ok();
    let code = headers
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(':').next())
        .filter(|code| !code.is_empty())
        .map(str::to_string)
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|body| body.get("__type"))
                .and_then(Value::as_str)
                .and_then(|value| value.rsplit('#').next())
                .filter(|code| !code.is_empty())
                .map(str::to_string)
        });
    let body_message = parsed
        .as_ref()
        .and_then(|body| body.get("message").or_else(|| body.get("Message")))
        .and_then(Value::as_str)
        .map(str::to_string);

    let (name, message, body) = match code {
        Some(code) => (
            Some(code),
            body_message.unwrap_or_else(|| "UnknownError".to_string()),
            None,
        ),
        None => (
            Some("Unknown".to_string()),
            "UnknownError".to_string(),
            (!trimmed.is_empty()).then(|| trimmed.to_string()),
        ),
    };
    BedrockFailure {
        name,
        service_exception: true,
        message,
        status: Some(status),
        body,
        metadata_request_id: normalize_diagnostic_value(
            headers
                .get("x-amzn-requestid")
                .and_then(|value| value.to_str().ok()),
        ),
    }
}

// =============================================================================
// Stream driver
// =============================================================================

fn run_stream(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: BedrockOptions,
) -> mpsc::Receiver<AssistantMessageEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        run_stream_task(cfg, model, ctx, options, tx).await;
    });
    rx
}

/// Which kind of streamed block is open (upstream `Block.type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireKind {
    Text,
    Thinking,
    Tool,
}

/// Streaming scratch for one wire content block (upstream stores `index`,
/// `partialJson`, and `redactedChunks` directly on the block objects; the
/// port's [`AssistantBlock`] carries no scratch, so it lives here keyed by
/// the wire `contentBlockIndex`).
struct WireBlock {
    wire_index: u64,
    content_index: usize,
    kind: WireKind,
    partial_json: String,
    redacted_chunks: Vec<Vec<u8>>,
}

struct StreamState {
    output: AssistantMessage,
    blocks: Vec<WireBlock>,
    /// Upstream `responseRequestId`: captured from the response metadata so
    /// the catch can correlate a mid-stream failure (exceptions delivered as
    /// stream events carry no HTTP metadata of their own).
    response_request_id: Option<String>,
}

impl StreamState {
    fn new(model: &Model) -> Self {
        StreamState {
            output: AssistantMessage {
                content: Vec::new(),
                api: API.to_string(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                provider_thinking_level: None,
                diagnostics: None,
                usage: crate::ai::types::Usage::default(),
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: now_ms(),
            },
            blocks: Vec::new(),
            response_request_id: None,
        }
    }
}

async fn run_stream_task(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: BedrockOptions,
    tx: mpsc::Sender<AssistantMessageEvent>,
) {
    let mut state = StreamState::new(&model);
    let signal = request_signal(&options.stream.signal);
    let outcome: Result<(), BedrockFailure> = async {
        let env = options.stream.env.clone();
        // Upstream lines 180-187: bearer token = options.bearerToken (no port
        // surface) || options.apiKey || AWS_BEARER_TOKEN_BEDROCK. The
        // provider-config key is the port's options.apiKey fallback.
        let config_api_key = options
            .stream
            .api_key
            .as_deref()
            .filter(|key| !key.is_empty())
            .or((!cfg.api_key.is_empty()).then_some(cfg.api_key.as_str()));
        let resolved = resolve_endpoint_config(
            &model,
            env.as_ref(),
            ambient_profile_configured(),
            config_api_key,
        );
        let payload = build_payload(&model, &ctx, &options)?;
        // Upstream lines 280-283: onPayload is awaited before the command is
        // built, so the replacement (not the built input) is what gets
        // serialized and signed; JS `undefined` preserves the built input
        // while any other value — including `null` — replaces it. A hook
        // rejection throws into the catch block before any HTTP traffic.
        let payload = options
            .stream
            .callbacks
            .payload(payload, &model)
            .await
            .map_err(|error| BedrockFailure::plain(error.to_string()))?;
        let body = serde_json::to_vec(&payload)
            .map_err(|error| BedrockFailure::plain(error.to_string()))?;
        let url = build_stream_url(&resolved, &model.id)
            .map_err(BedrockFailure::plain)?;

        // Upstream lines 256-259: caller headers attach before signing;
        // reserved SigV4/auth headers are silently skipped.
        let mut headers: Vec<(String, String)> =
            vec![("content-type".to_string(), "application/json".to_string())];
        if let Some(custom) = &options.stream.headers {
            for (key, value) in custom {
                if let Some(value) = value {
                    if !is_reserved_header(key) {
                        set_header(&mut headers, key, value);
                    }
                }
            }
        }

        let auth_headers = if resolved.use_bearer {
            vec![bearer_authorization(resolved.bearer_token.as_deref().unwrap_or_default())]
        } else {
            let credentials = resolved.credentials.clone().ok_or_else(|| {
                BedrockFailure::plain("Could not load credentials from any providers")
            })?;
            let region = resolved.region.clone().ok_or_else(|| {
                BedrockFailure::plain(
                    "Unable to determine a region from the configured sources. Please configure a region or profile.",
                )
            })?;
            sign_request(
                "POST",
                &url,
                &headers,
                &body,
                &region,
                &credentials,
                std::time::SystemTime::now(),
            )
            .map_err(BedrockFailure::plain)?
        };
        headers.extend(auth_headers);

        let response = send_stream_request(&url, headers, body, &options.stream, &signal).await?;
        // Upstream lines 251-255 + 510-526: with onResponse set, the
        // deserialize-step middleware fires with the raw Smithy HTTP response
        // (status and raw headers) once the send resolves, before the event
        // stream is consumed; a hook rejection throws into the catch block.
        // (The `$metadata` fallback at lines 288-294 only runs when the
        // middleware did not observe a raw response — unreachable with the
        // real SDK — so the raw-response callback is the wired surface.)
        options
            .stream
            .callbacks
            .http_response(&response, &model)
            .await
            .map_err(|error| BedrockFailure::plain(error.to_string()))?;
        state.response_request_id = normalize_diagnostic_value(
            response
                .headers()
                .get("x-amzn-requestid")
                .and_then(|value| value.to_str().ok()),
        );

        consume_event_stream(&mut state, response, &model, &signal, &tx).await?;

        // Upstream line 330: the post-stream abort check precedes the
        // pending / error guards.
        if signal.is_cancelled() {
            return Err(BedrockFailure::plain(REQUEST_WAS_ABORTED));
        }
        if state.output.stop_reason == StopReason::Pending {
            return Err(BedrockFailure::plain(
                "Bedrock stream ended without a stop reason",
            ));
        }
        if matches!(
            state.output.stop_reason,
            StopReason::Error | StopReason::Aborted
        ) {
            return Err(BedrockFailure::plain(
                state
                    .output
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "An unknown error occurred".to_string()),
            ));
        }

        // A stream can settle without stopping every block, so finalize here
        // too (upstream line 342).
        finalize_blocks(&mut state);
        let reason = match state.output.stop_reason {
            StopReason::Length => SuccessReason::Length,
            StopReason::ToolUse => SuccessReason::ToolUse,
            _ => SuccessReason::Stop,
        };
        let _ = tx
            .send(AssistantMessageEvent::Done {
                reason,
                message: state.output.clone(),
            })
            .await;
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => {}
        Err(failure) => {
            // Upstream catch block (lines 345-356): finalize, settle the stop
            // reason ("aborted" when the request signal fired, else "error"),
            // format the error, and attach the failure diagnostic for error
            // stops.
            let aborted = signal.is_cancelled();
            finalize_blocks(&mut state);
            state.output.stop_reason = if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            state.output.error_message = Some(format_bedrock_error(&failure));
            if !aborted {
                append_bedrock_failure_diagnostic(
                    &mut state.output,
                    &failure,
                    state.response_request_id.as_deref(),
                );
            }
            let _ = tx
                .send(AssistantMessageEvent::Error {
                    reason: if aborted {
                        ErrorReason::Aborted
                    } else {
                        ErrorReason::Error
                    },
                    error: state.output,
                })
                .await;
        }
    }
}

/// `Boolean(getProviderEnvValue("AWS_PROFILE"))` — ambient process env only.
fn ambient_profile_configured() -> bool {
    std::env::var("AWS_PROFILE").is_ok_and(|value| !value.is_empty())
}

/// Upstream lines 286-294: the send plus the non-2xx → service exception
/// mapping, wrapped in the provider-request retry seam (initial request
/// only, `maxRetries` default 0).
async fn send_stream_request(
    url: &str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    stream_options: &StreamOptions,
    signal: &CancellationToken,
) -> Result<reqwest::Response, BedrockFailure> {
    let mut header_map = reqwest::header::HeaderMap::new();
    for (name, value) in &headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            BedrockFailure::plain(format!("Invalid header name \"{name}\": {error}"))
        })?;
        let value = reqwest::header::HeaderValue::from_str(value).map_err(|error| {
            BedrockFailure::plain(format!("Invalid header value for \"{name}\": {error}"))
        })?;
        header_map.insert(name, value);
    }
    let mut request = http_client()
        .post(url)
        .headers(header_map)
        .body(body.clone());
    if let Some(ms) = stream_options.timeout_ms {
        request = request.timeout(Duration::from_millis(ms));
    }
    let max_retries = stream_options.max_retries.unwrap_or(0);
    let max_retry_delay_ms = stream_options.max_retry_delay_ms;
    // Non-2xx failures ride the seam as ProviderErrors (the policy decides
    // retryability); the structured failure is stashed alongside so the
    // caller can surface it verbatim when the seam gives up.
    let failure_slot: std::sync::Mutex<Option<BedrockFailure>> = std::sync::Mutex::new(None);
    let outcome = retry_provider_request(max_retries, max_retry_delay_ms, Some(signal), || {
        let request = request
            .try_clone()
            .expect("JSON request body is buffered and clonable");
        let failure_slot = &failure_slot;
        async move {
            let response = request
                .send()
                .await
                .map_err(|error| ProviderError::transport(format_transport_error(&error)))?;
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            let status_code = status.as_u16();
            let response_headers = response.headers().clone();
            let body_text = response.text().await.unwrap_or_default();
            let failure = http_failure(status_code, &response_headers, &body_text);
            if let Ok(mut slot) = failure_slot.lock() {
                *slot = Some(failure.clone());
            }
            Err(ProviderError::http(
                status_code,
                response_headers,
                failure.message.clone(),
            ))
        }
    })
    .await;
    match outcome {
        Ok(response) => Ok(response),
        Err(error) => Err(failure_slot
            .into_inner()
            .ok()
            .flatten()
            .unwrap_or_else(|| BedrockFailure::plain(error.message))),
    }
}

/// Transport-failure messages; upstream surfaces the AbortSignal.timeout
/// DOMException text for timeouts (reqwest's Display omits the cause).
fn format_transport_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "The operation was aborted due to timeout".to_string();
    }
    error.to_string()
}

/// The event-stream consume loop (upstream lines 296-328): decode frames,
/// then dispatch each by `:event-type` / `:message-type`. The SDK's
/// `abortSignal` breaks body reads on abort; the select breaks the read the
/// moment the token cancels.
async fn consume_event_stream(
    state: &mut StreamState,
    response: reqwest::Response,
    model: &Model,
    signal: &CancellationToken,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), BedrockFailure> {
    let mut decoder = FrameDecoder::new();
    let mut chunks = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = signal.cancelled() => {
                return Err(BedrockFailure::plain(REQUEST_WAS_ABORTED));
            }
            next = chunks.next() => match next {
                Some(chunk) => chunk,
                None => break,
            },
        };
        let chunk = chunk.map_err(|error| BedrockFailure::plain(error.to_string()))?;
        for frame in decoder.decode(&chunk).map_err(BedrockFailure::plain)? {
            dispatch_frame(state, frame, model, tx).await?;
        }
    }
    if signal.is_cancelled() {
        return Err(BedrockFailure::plain(REQUEST_WAS_ABORTED));
    }
    decoder.finish().map_err(BedrockFailure::plain)
}

/// Dispatches one decoded frame. Event payloads are the union member's own
/// JSON object; the member key is the frame's `:event-type`.
async fn dispatch_frame(
    state: &mut StreamState,
    frame: Frame,
    model: &Model,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), BedrockFailure> {
    match frame.message_type.as_str() {
        "event" => {
            let Some(event_type) = frame.event_type.as_deref() else {
                return Ok(());
            };
            let payload: Value = if frame.payload.is_empty() {
                Value::Object(Map::new())
            } else {
                serde_json::from_slice(&frame.payload)
                    .map_err(|error| BedrockFailure::plain(error.to_string()))?
            };
            handle_event(state, event_type, &payload, model, tx).await
        }
        // Exception frames (modeled and unmodeled alike) reach upstream's
        // catch as bare deserialized objects: no `.name`, no `$metadata`, so
        // no prefix and no error code — only the serialized payload.
        "exception" => Err(BedrockFailure {
            name: None,
            service_exception: false,
            message: payload_to_string(&frame.payload),
            status: None,
            body: None,
            metadata_request_id: None,
        }),
        // The SDK throws a real `Error` named after the frame's
        // `:error-code` here (upstream comment, lines 142-146).
        "error" => Err(BedrockFailure {
            name: frame.error_code,
            service_exception: false,
            message: frame.error_message.unwrap_or_default(),
            status: None,
            body: None,
            metadata_request_id: None,
        }),
        _ => Ok(()),
    }
}

/// `safeJsonStringify` for exception payloads: objects serialize as JSON,
/// non-JSON payloads surface as lossy text.
fn payload_to_string(payload: &[u8]) -> String {
    match serde_json::from_slice::<Value>(payload) {
        Ok(value) => value.to_string(),
        Err(_) => String::from_utf8_lossy(payload).into_owned(),
    }
}

/// One ConverseStream event (upstream loop body, lines 296-328).
async fn handle_event(
    state: &mut StreamState,
    event_type: &str,
    payload: &Value,
    model: &Model,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), BedrockFailure> {
    match event_type {
        "messageStart" => {
            if payload.get("role").and_then(Value::as_str) != Some("assistant") {
                return Err(BedrockFailure::plain(
                    "Unexpected assistant message start but got user message start instead",
                ));
            }
            let _ = tx
                .send(AssistantMessageEvent::Start {
                    message: state.output.clone(),
                })
                .await;
        }
        "contentBlockStart" => handle_content_block_start(state, payload, tx).await,
        "contentBlockDelta" => handle_content_block_delta(state, payload, tx).await,
        "contentBlockStop" => handle_content_block_stop(state, payload, tx).await,
        "messageStop" => handle_message_stop(state, payload),
        "metadata" => handle_metadata(state, payload, model),
        // Unknown union members are ignored (the upstream if/else chain falls
        // through for anything it does not model).
        _ => {}
    }
    Ok(())
}

/// Upstream `handleContentBlockStart` (lines 579-600): only tool-use starts
/// open a block; text blocks are created lazily by their first delta.
async fn handle_content_block_start(
    state: &mut StreamState,
    payload: &Value,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    let Some(index) = payload.get("contentBlockIndex").and_then(Value::as_u64) else {
        return;
    };
    let tool_use = payload
        .get("start")
        .and_then(|start| start.get("toolUse"))
        .filter(|value| value.is_object());
    let Some(tool_use) = tool_use else {
        return;
    };
    let call = ToolCall {
        id: tool_use
            .get("toolUseId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        name: tool_use
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        arguments: json!({}),
        thought_signature: None,
        namespace: None,
    };
    state.output.content.push(AssistantBlock::ToolCall(call));
    let content_index = state.output.content.len() - 1;
    state.blocks.push(WireBlock {
        wire_index: index,
        content_index,
        kind: WireKind::Tool,
        partial_json: String::new(),
        redacted_chunks: Vec::new(),
    });
    let _ = tx
        .send(AssistantMessageEvent::ToolcallStart { content_index })
        .await;
}

/// Upstream `handleContentBlockDelta` (lines 602-678).
async fn handle_content_block_delta(
    state: &mut StreamState,
    payload: &Value,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    let content_block_index = payload
        .get("contentBlockIndex")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let Some(delta) = payload.get("delta") else {
        return;
    };

    if let Some(text) = delta.get("text").and_then(Value::as_str) {
        // Text blocks have no contentBlockStart; create one lazily.
        let content_index = match state
            .blocks
            .iter()
            .find(|block| block.wire_index == content_block_index)
        {
            Some(block) => block.content_index,
            None => {
                state.output.content.push(AssistantBlock::Text(TextContent {
                    text: String::new(),
                    text_signature: None,
                }));
                let content_index = state.output.content.len() - 1;
                state.blocks.push(WireBlock {
                    wire_index: content_block_index,
                    content_index,
                    kind: WireKind::Text,
                    partial_json: String::new(),
                    redacted_chunks: Vec::new(),
                });
                let _ = tx
                    .send(AssistantMessageEvent::TextStart { content_index })
                    .await;
                content_index
            }
        };
        // Only text blocks consume text deltas (upstream `block.type === "text"`).
        if state
            .blocks
            .iter()
            .any(|block| block.wire_index == content_block_index && block.kind == WireKind::Text)
        {
            if let Some(AssistantBlock::Text(text_block)) =
                state.output.content.get_mut(content_index)
            {
                text_block.text.push_str(text);
            }
            let _ = tx
                .send(AssistantMessageEvent::TextDelta {
                    content_index,
                    delta: text.to_string(),
                })
                .await;
        }
    } else if let Some(tool_use) = delta.get("toolUse").filter(|value| value.is_object()) {
        // `delta?.toolUse && block?.type === "toolCall"` — silently skipped
        // when no tool block is open at the index.
        let open = state
            .blocks
            .iter()
            .find(|block| block.wire_index == content_block_index)
            .filter(|block| block.kind == WireKind::Tool)
            .map(|block| block.content_index);
        let Some(content_index) = open else {
            return;
        };
        // `delta.toolUse.input || ""`.
        let input = tool_use.get("input").and_then(Value::as_str).unwrap_or("");
        let arguments = {
            let Some(block) = state
                .blocks
                .iter_mut()
                .find(|block| block.wire_index == content_block_index)
            else {
                return;
            };
            block.partial_json.push_str(input);
            parse_streaming_json(&block.partial_json)
        };
        if let Some(AssistantBlock::ToolCall(call)) = state.output.content.get_mut(content_index) {
            call.arguments = arguments;
        }
        let _ = tx
            .send(AssistantMessageEvent::ToolcallDelta {
                content_index,
                delta: input.to_string(),
            })
            .await;
    } else if let Some(reasoning) = delta
        .get("reasoningContent")
        .filter(|value| value.is_object())
    {
        handle_reasoning_delta(state, content_block_index, reasoning, tx).await;
    }
}

/// The `reasoningContent` arm (upstream lines 630-677): signed thinking text
/// plus encrypted `redactedContent` payloads.
async fn handle_reasoning_delta(
    state: &mut StreamState,
    content_block_index: u64,
    reasoning: &Value,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    // Lazily open the thinking block (upstream creates it on the first
    // reasoning delta even without a contentBlockStart).
    let content_index = match state
        .blocks
        .iter()
        .find(|block| block.wire_index == content_block_index)
    {
        Some(block) => block.content_index,
        None => {
            state
                .output
                .content
                .push(AssistantBlock::Thinking(ThinkingContent {
                    thinking: String::new(),
                    thinking_signature: Some(String::new()),
                    redacted: None,
                }));
            let content_index = state.output.content.len() - 1;
            state.blocks.push(WireBlock {
                wire_index: content_block_index,
                content_index,
                kind: WireKind::Thinking,
                partial_json: String::new(),
                redacted_chunks: Vec::new(),
            });
            let _ = tx
                .send(AssistantMessageEvent::ThinkingStart { content_index })
                .await;
            content_index
        }
    };
    // Only thinking blocks consume reasoning deltas.
    if !state
        .blocks
        .iter()
        .any(|block| block.wire_index == content_block_index && block.kind == WireKind::Thinking)
    {
        return;
    }

    // `if (delta.reasoningContent.text)` — truthy (non-empty) text only.
    if let Some(text) = reasoning
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        if let Some(AssistantBlock::Thinking(thinking)) =
            state.output.content.get_mut(content_index)
        {
            thinking.thinking.push_str(text);
        }
        let _ = tx
            .send(AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta: text.to_string(),
            })
            .await;
    }

    // `thinkingSignature` holds either an Anthropic signature or an opaque
    // redacted payload, never both: mixing them corrupts whichever arrived
    // first (upstream `signature && !thinkingBlock.redacted`).
    let redacted = state
        .output
        .content
        .get(content_index)
        .map(|block| {
            matches!(block, AssistantBlock::Thinking(thinking) if thinking.redacted == Some(true))
        })
        .unwrap_or(false);
    if let Some(signature) = reasoning
        .get("signature")
        .and_then(Value::as_str)
        .filter(|signature| !signature.is_empty())
    {
        if !redacted {
            if let Some(AssistantBlock::Thinking(thinking)) =
                state.output.content.get_mut(content_index)
            {
                let existing = thinking.thinking_signature.get_or_insert_with(String::new);
                existing.push_str(signature);
            }
        }
    }

    // Encrypted reasoning from non-Anthropic models on Bedrock (e.g. OpenAI
    // GPT-5.6): the payload is opaque, so keep it verbatim in
    // `thinkingSignature` the way the Anthropic path stores redacted
    // thinking, and replay it on the next turn.
    if let Some(redacted_content) = reasoning
        .get("redactedContent")
        .and_then(Value::as_str)
        .and_then(|encoded| base64_decode(encoded).ok())
        .filter(|bytes| !bytes.is_empty())
    {
        if !redacted {
            if let Some(AssistantBlock::Thinking(thinking)) =
                state.output.content.get_mut(content_index)
            {
                thinking.redacted = Some(true);
                thinking.thinking_signature = Some(String::new());
                thinking.thinking.push_str(REDACTED_THINKING_PLACEHOLDER);
                let _ = tx
                    .send(AssistantMessageEvent::ThinkingDelta {
                        content_index,
                        delta: REDACTED_THINKING_PLACEHOLDER.to_string(),
                    })
                    .await;
            }
        }
        if let Some(block) = state
            .blocks
            .iter_mut()
            .find(|block| block.wire_index == content_block_index)
        {
            block.redacted_chunks.push(redacted_content);
        }
    }
}

/// Upstream `handleMetadata` (lines 702-719): usage accounting with the 1h
/// cache-write split, then cost recomputation.
fn handle_metadata(state: &mut StreamState, payload: &Value, model: &Model) {
    let Some(usage) = payload.get("usage") else {
        return;
    };
    let output = &mut state.output.usage;
    output.input = usage
        .get("inputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    output.output = usage
        .get("outputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    output.cache_read = usage
        .get("cacheReadInputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    output.cache_write = usage
        .get("cacheWriteInputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    output.cache_write_1h = usage.get("cacheDetails").map(|details| {
        details
            .as_array()
            .map(|details| {
                details
                    .iter()
                    .filter(|detail| detail.get("ttl").and_then(Value::as_str) == Some("1h"))
                    .map(|detail| {
                        detail
                            .get("inputTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                    })
                    .fold(0u64, |total, tokens| total.saturating_add(tokens))
            })
            .unwrap_or(0)
    });
    output.total_tokens = usage
        .get("totalTokens")
        .and_then(Value::as_u64)
        .filter(|total| *total > 0)
        .unwrap_or_else(|| output.input + output.output);
    calculate_cost(model, &mut state.output.usage);
}

/// Upstream `handleContentBlockStop` (lines 721-748): finalizes the block
/// and strips its wire index, so later deltas for the same index start a
/// fresh block (upstream `delete block.index`).
async fn handle_content_block_stop(
    state: &mut StreamState,
    payload: &Value,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    let Some(content_block_index) = payload.get("contentBlockIndex").and_then(Value::as_u64) else {
        return;
    };
    let Some(position) = state
        .blocks
        .iter()
        .position(|block| block.wire_index == content_block_index)
    else {
        return;
    };
    let mut block = state.blocks.remove(position);
    match block.kind {
        WireKind::Text => {
            let content = match state.output.content.get(block.content_index) {
                Some(AssistantBlock::Text(text)) => text.text.clone(),
                _ => String::new(),
            };
            let _ = tx
                .send(AssistantMessageEvent::TextEnd {
                    content_index: block.content_index,
                    content,
                })
                .await;
        }
        WireKind::Thinking => {
            flush_redacted_content(&mut block, &mut state.output);
            let content = match state.output.content.get(block.content_index) {
                Some(AssistantBlock::Thinking(thinking)) => thinking.thinking.clone(),
                _ => String::new(),
            };
            let _ = tx
                .send(AssistantMessageEvent::ThinkingEnd {
                    content_index: block.content_index,
                    content,
                })
                .await;
        }
        WireKind::Tool => {
            // Finalize in place and strip the scratch buffer so replay only
            // carries parsed arguments.
            let arguments = parse_streaming_json(&block.partial_json);
            if let Some(AssistantBlock::ToolCall(call)) =
                state.output.content.get_mut(block.content_index)
            {
                call.arguments = arguments;
            }
            let tool_call = match state.output.content.get(block.content_index) {
                Some(AssistantBlock::ToolCall(call)) => call.clone(),
                _ => ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: json!({}),
                    thought_signature: None,
                    namespace: None,
                },
            };
            let _ = tx
                .send(AssistantMessageEvent::ToolcallEnd {
                    content_index: block.content_index,
                    tool_call,
                })
                .await;
        }
    }
}

/// Upstream `flushRedactedContent` (lines 685-689): encode the buffered
/// encrypted-reasoning chunks into `thinkingSignature` and drop the scratch
/// buffer, which must never reach a persisted message.
fn flush_redacted_content(block: &mut WireBlock, output: &mut AssistantMessage) {
    if block.redacted_chunks.is_empty() {
        return;
    }
    let chunks = std::mem::take(&mut block.redacted_chunks);
    let flat: Vec<u8> = chunks.concat();
    if let Some(AssistantBlock::Thinking(thinking)) = output.content.get_mut(block.content_index) {
        thinking.thinking_signature = Some(base64_encode(&flat));
    }
}

/// Upstream `finalizeStreamingBlock` over every block (lines 342, 346-348):
/// strips streaming scratch; in the port that means flushing the redacted
/// chunks and parsing the arguments of tool blocks the stream never stopped.
fn finalize_blocks(state: &mut StreamState) {
    let mut blocks = std::mem::take(&mut state.blocks);
    for block in &mut blocks {
        if block.kind == WireKind::Tool && !block.partial_json.is_empty() {
            let arguments = parse_streaming_json(&block.partial_json);
            if let Some(AssistantBlock::ToolCall(call)) =
                state.output.content.get_mut(block.content_index)
            {
                call.arguments = arguments;
            }
        }
        flush_redacted_content(block, &mut state.output);
    }
}

/// Upstream `mapStopReason` (lines 1158-1173). Note upstream's falsy check:
/// an empty-string stop reason maps to a bare error like a missing one.
fn map_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
    match reason {
        Some("end_turn") | Some("stop_sequence") => (StopReason::Stop, None),
        Some("max_tokens") | Some("model_context_window_exceeded") => (StopReason::Length, None),
        Some("tool_use") => (StopReason::ToolUse, None),
        Some(reason) if !reason.is_empty() => (
            StopReason::Error,
            Some(format!("Provider stopped with: {reason}")),
        ),
        _ => (StopReason::Error, None),
    }
}

/// Upstream lines 308-314: the messageStop handler.
fn handle_message_stop(state: &mut StreamState, payload: &Value) {
    let reason = payload
        .get("stopReason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty());
    state.output.raw_stop_reason = reason.map(str::to_string);
    let (stop_reason, error_message) = map_stop_reason(reason);
    state.output.stop_reason = stop_reason;
    if let Some(error_message) = error_message {
        state.output.error_message = Some(error_message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::bedrock::event_stream::crc32;
    use crate::ai::api::REQUEST_ABORTED;
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::events::PartialAssistant;
    use crate::ai::types::message::UserMessage;
    use crate::ai::types::primitives::{ModelCost, ToolChoice};
    use crate::ai::types::tool::{ConstrainedSampling, JsonSchemaSampling, Strict};
    use crate::ai::types::ModelInput;
    use std::collections::BTreeMap;

    const TS: i64 = 1758240000000;

    // =========================================================================
    // Fixtures and wire helpers
    // =========================================================================

    fn env_map(pairs: &[(&str, &str)]) -> ProviderEnv {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<String, String>>()
    }

    fn model(id: &str, name: &str) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: id.to_string(),
            name: name.to_string(),
            api: API.to_string(),
            provider: "amazon-bedrock".to_string(),
            base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![ModelInput::Text, ModelInput::Image],
            cost: ModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
                tiers: None,
            },
            context_window: 200000,
            max_tokens: 64000,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: Some(json!({"supportsStrictMode": true})),
        }
    }

    /// The oracle base model: Claude Sonnet 4.5 on the US inference profile.
    fn claude_sonnet_4_5() -> Model {
        model(
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "Claude Sonnet 4.5 (US)",
        )
    }

    fn nova_lite() -> Model {
        let mut nova = model("amazon.nova-lite-v1:0", "Nova Lite");
        nova.reasoning = false;
        nova.compat = None;
        nova
    }

    fn cfg() -> ProviderConfig {
        ProviderConfig {
            base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            api_key: String::new(),
            max_tokens: 4096,
        }
    }

    fn user_context(content: &str) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text(content.to_string()),
                timestamp: TS,
            })],
            tools: None,
        })
    }

    /// One event-stream wire frame: prelude + headers + payload + CRCs.
    fn wire_frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
        let mut header_bytes = Vec::new();
        for (name, value) in headers {
            header_bytes.push(name.len() as u8);
            header_bytes.extend_from_slice(name.as_bytes());
            header_bytes.push(7); // string header
            header_bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
            header_bytes.extend_from_slice(value.as_bytes());
        }
        let total = 12 + header_bytes.len() + payload.len() + 4;
        let mut frame = Vec::with_capacity(total);
        frame.extend_from_slice(&(total as u32).to_be_bytes());
        frame.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
        frame.extend_from_slice(&crc32(&frame).to_be_bytes());
        frame.extend_from_slice(&header_bytes);
        frame.extend_from_slice(payload);
        frame.extend_from_slice(&crc32(&frame).to_be_bytes());
        frame
    }

    fn event_frame(event_type: &str, payload: &Value) -> Vec<u8> {
        wire_frame(
            &[
                (":message-type", "event"),
                (":event-type", event_type),
                (":content-type", "application/json"),
            ],
            payload.to_string().as_bytes(),
        )
    }

    fn exception_frame(exception_type: &str, payload: &Value) -> Vec<u8> {
        wire_frame(
            &[
                (":message-type", "exception"),
                (":exception-type", exception_type),
                (":content-type", "application/json"),
            ],
            payload.to_string().as_bytes(),
        )
    }

    fn error_frame(code: &str, message: &str) -> Vec<u8> {
        wire_frame(
            &[
                (":message-type", "error"),
                (":error-code", code),
                (":error-message", message),
            ],
            b"",
        )
    }

    async fn serve_bytes(server: &wiremock::MockServer, status: u16, body: Vec<u8>) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(status)
                    .set_body_bytes(body)
                    .insert_header("content-type", "application/vnd.amazon.eventstream"),
            )
            .mount(server)
            .await;
    }

    /// Serves one 200 response whose body is the given frames. The response
    /// carries `x-amzn-requestid: req-123`, mirroring the oracle mocks'
    /// `$metadata.requestId`.
    async fn serve_frames(server: &wiremock::MockServer, frames: &[Vec<u8>]) {
        let mut body = Vec::new();
        for frame in frames {
            body.extend_from_slice(frame);
        }
        let mut template = wiremock::ResponseTemplate::new(200)
            .set_body_bytes(body)
            .insert_header("content-type", "application/vnd.amazon.eventstream");
        template = template.insert_header("x-amzn-requestid", "req-123");
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(template)
            .mount(server)
            .await;
    }

    /// Runs one stream and returns (events, request) for the single request.
    /// The model's base URL is redirected to the mock server (a custom
    /// endpoint, which endpoint pinning always passes through) so no test
    /// traffic leaves the machine.
    async fn capture_stream(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> (Vec<AssistantMessageEvent>, wiremock::Request) {
        let mut model = model.clone();
        model.base_url = server.uri();
        let api = BedrockConverseStream;
        let mut rx = api.stream(&cfg(), &model, ctx, options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        let requests = server.received_requests().await.unwrap();
        assert!(!requests.is_empty(), "at least one request expected");
        (out, requests.last().unwrap().clone())
    }

    async fn capture_simple(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> (Vec<AssistantMessageEvent>, wiremock::Request) {
        let mut model = model.clone();
        model.base_url = server.uri();
        let api = BedrockConverseStream;
        let mut rx = api.stream_simple(&cfg(), &model, ctx, options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        let requests = server.received_requests().await.unwrap();
        assert!(!requests.is_empty(), "at least one request expected");
        (out, requests.last().unwrap().clone())
    }

    fn request_body(request: &wiremock::Request) -> Value {
        serde_json::from_slice(&request.body).unwrap()
    }

    /// The content array of the first assistant message in a request body.
    fn assistant_content(body: &Value) -> Value {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == json!("assistant"))
            .unwrap()["content"]
            .clone()
    }

    fn event_types(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(AssistantMessageEvent::event_type)
            .collect()
    }

    /// Terminal error event, asserting the stream ended with exactly one.
    fn error_of(events: &[AssistantMessageEvent]) -> AssistantMessage {
        match events.last() {
            Some(AssistantMessageEvent::Error { error, .. }) => {
                assert_eq!(event_types(events).last(), Some(&"error"));
                error.clone()
            }
            other => panic!("expected terminal error, got {other:?}"),
        }
    }

    /// Terminal done event.
    fn done_of(events: &[AssistantMessageEvent]) -> (SuccessReason, AssistantMessage) {
        match events.last() {
            Some(AssistantMessageEvent::Done { reason, message }) => (*reason, message.clone()),
            other => panic!("expected terminal done, got {other:?}"),
        }
    }

    fn apply_all(events: &[AssistantMessageEvent]) -> PartialAssistant {
        let mut partial = PartialAssistant::new();
        for event in events {
            partial
                .apply(event)
                .unwrap_or_else(|error| panic!("reducer rejected {}: {error}", event.event_type()));
        }
        partial
    }

    fn diagnostic_of(message: &AssistantMessage) -> &Value {
        let diagnostics = message.diagnostics.as_ref().unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].r#type, "bedrock_response_failure");
        assert!(diagnostics[0].error.is_none());
        diagnostics[0].details.as_ref().unwrap()
    }

    fn header_value<'a>(request: &'a wiremock::Request, name: &str) -> &'a str {
        request
            .headers
            .get(name)
            .unwrap_or_else(|| panic!("header {name} missing"))
            .to_str()
            .unwrap()
    }

    fn text_block(text: &str) -> AssistantBlock {
        AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    }

    fn thinking_block(thinking: &str, signature: Option<&str>, redacted: bool) -> AssistantBlock {
        AssistantBlock::Thinking(ThinkingContent {
            thinking: thinking.to_string(),
            thinking_signature: signature.map(str::to_string),
            redacted: redacted.then_some(true),
        })
    }

    fn assistant_message(
        model: &Model,
        content: Vec<AssistantBlock>,
        stop_reason: StopReason,
    ) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            api: API.to_string(),
            provider: "amazon-bedrock".to_string(),
            model: model.id.clone(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: crate::ai::types::Usage::default(),
            stop_reason,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: TS,
        })
    }

    /// Standard stream fixture: start, one text delta, stop, end_turn.
    fn text_stream_frames(text: &str, stop_reason: &str) -> Vec<Vec<u8>> {
        vec![
            event_frame("messageStart", &json!({ "role": "assistant" })),
            event_frame(
                "contentBlockDelta",
                &json!({ "contentBlockIndex": 0, "delta": { "text": text } }),
            ),
            event_frame("contentBlockStop", &json!({ "contentBlockIndex": 0 })),
            event_frame("messageStop", &json!({ "stopReason": stop_reason })),
        ]
    }

    /// Pins the scoped env used by wire tests: credentials resolve from the
    /// scoped map (which wins over the process environment), so the tests are
    /// hermetic on machines exporting real AWS_* variables.
    fn signing_env(region: &str) -> Option<ProviderEnv> {
        Some(env_map(&[
            ("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE"),
            (
                "AWS_SECRET_ACCESS_KEY",
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            ),
            ("AWS_REGION", region),
        ]))
    }

    fn stream_options(env: Option<ProviderEnv>) -> StreamOptions {
        StreamOptions {
            cache_retention: Some(CacheRetention::None),
            env,
            ..StreamOptions::default()
        }
    }

    /// Default (short) cache retention — the upstream default.
    fn short_retention_options(env: Option<ProviderEnv>) -> StreamOptions {
        StreamOptions {
            cache_retention: Some(CacheRetention::Short),
            env,
            ..StreamOptions::default()
        }
    }

    // =========================================================================
    // Endpoint / region resolution (endpoint-resolution oracle)
    // =========================================================================

    #[test]
    fn standard_endpoint_region_extraction() {
        assert_eq!(
            get_standard_bedrock_endpoint_region(
                "https://bedrock-runtime.eu-central-1.amazonaws.com"
            ),
            Some("eu-central-1".to_string())
        );
        assert_eq!(
            get_standard_bedrock_endpoint_region(
                "https://bedrock-runtime-fips.us-gov-west-1.amazonaws.com"
            ),
            Some("us-gov-west-1".to_string())
        );
        assert_eq!(
            get_standard_bedrock_endpoint_region(
                "https://bedrock-runtime.cn-north-1.amazonaws.com.cn"
            ),
            Some("cn-north-1".to_string())
        );
        // Custom hosts are not standard endpoints.
        assert_eq!(
            get_standard_bedrock_endpoint_region("http://127.0.0.1:8080"),
            None
        );
        assert_eq!(
            get_standard_bedrock_endpoint_region("https://bedrock-runtime.evil.example.com"),
            None
        );
    }

    #[test]
    fn explicit_endpoint_matrix() {
        let standard = "https://bedrock-runtime.eu-central-1.amazonaws.com";
        // Custom endpoints always pass through.
        assert!(should_use_explicit_bedrock_endpoint(
            "https://bedrock-vpc.example.com",
            Some("us-east-1"),
            true
        ));
        // Standard endpoints are pinned only without region and ambient profile.
        assert!(should_use_explicit_bedrock_endpoint(standard, None, false));
        assert!(!should_use_explicit_bedrock_endpoint(
            standard,
            Some("us-east-2"),
            false
        ));
        assert!(!should_use_explicit_bedrock_endpoint(standard, None, true));
    }

    #[test]
    fn arn_region_extraction() {
        assert_eq!(
            extract_arn_region(
                "arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/abc123"
            ),
            Some("us-west-2".to_string())
        );
        assert_eq!(
            extract_arn_region(
                "arn:aws-us-gov:bedrock:us-gov-west-1:123456789012:application-inference-profile/abc123"
            ),
            Some("us-gov-west-1".to_string())
        );
        // Any `arn:aws-<partition>` matches the upstream regex, not just
        // `-us-gov`.
        assert_eq!(
            extract_arn_region(
                "arn:aws-cn:bedrock:cn-north-1:123456789012:application-inference-profile/abc123"
            ),
            Some("cn-north-1".to_string())
        );
        assert_eq!(
            extract_arn_region(
                "arn:aws-iso-b:bedrock:us-iso-east-1:123456789012:application-inference-profile/abc"
            ),
            Some("us-iso-east-1".to_string())
        );
        // Non-ARN ids and malformed partitions do not match (upstream regex
        // requires a non-empty `[a-z0-9-]+` partition; unknown-but-well-formed
        // partitions like `usgov` do match, by design). Only `:bedrock:` is a
        // Bedrock ARN.
        assert_eq!(extract_arn_region("us.anthropic.claude-sonnet-4-5"), None);
        assert_eq!(extract_arn_region("arn:aws-:bedrock:us-east-1:1:x"), None);
        assert_eq!(extract_arn_region("arn:aws-US_GOV:bedrock:x:1:x"), None);
        assert_eq!(extract_arn_region("arn:aws:ec2:us-east-1:1:x"), None);
    }

    #[test]
    fn resolution_assigns_eu_region_from_endpoint_without_env() {
        let _env = cleared_aws_env();
        let mut model = model(
            "eu.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "Claude Sonnet 4.5 (EU)",
        );
        model.base_url = "https://bedrock-runtime.eu-central-1.amazonaws.com".to_string();
        let resolved = resolve_endpoint_config(&model, None, false, None);
        assert_eq!(resolved.endpoint.as_deref(), Some(model.base_url.as_str()));
        assert_eq!(resolved.region.as_deref(), Some("eu-central-1"));
        assert_eq!(resolved.profile, None);
    }

    #[test]
    fn resolution_does_not_pin_standard_endpoint_when_region_configured() {
        let env = env_map(&[("AWS_REGION", "us-east-2")]);
        let resolved = resolve_endpoint_config(&claude_sonnet_4_5(), Some(&env), false, None);
        assert_eq!(resolved.region.as_deref(), Some("us-east-2"));
        assert_eq!(resolved.endpoint, None);
    }

    #[test]
    fn resolution_profile_handling_matches_upstream() {
        let _env = cleared_aws_env();
        let mut model = model(
            "eu.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "Claude Sonnet 4.5 (EU)",
        );
        model.base_url = "https://bedrock-runtime.eu-central-1.amazonaws.com".to_string();

        // Scoped AWS_PROFILE: profile recorded, endpoint pinned, region from
        // the endpoint, and env credentials suppressed (`!optionsProfile`).
        let scoped = env_map(&[
            ("AWS_PROFILE", "scoped-bedrock-profile"),
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "SECRET"),
        ]);
        let resolved = resolve_endpoint_config(&model, Some(&scoped), false, None);
        assert_eq!(resolved.profile.as_deref(), Some("scoped-bedrock-profile"));
        assert_eq!(resolved.endpoint.as_deref(), Some(model.base_url.as_str()));
        assert_eq!(resolved.region.as_deref(), Some("eu-central-1"));
        assert_eq!(resolved.credentials, None);

        // Ambient AWS_PROFILE (process env, injected flag): no endpoint and no
        // region — the profile chain owns both (profile-file loading is SDK
        // behavior the port does not reproduce).
        let resolved = resolve_endpoint_config(&model, None, true, None);
        assert_eq!(resolved.endpoint, None);
        assert_eq!(resolved.region, None);
    }

    /// Process env is process-global; the shared
    /// [`test_support::TestEnv`](crate::ai::api::test_support::TestEnv)
    /// serializes env-mutating tests and restores the saved values on drop
    /// (the oracle's `stubEnv`/`afterEach`).
    use crate::ai::api::test_support::TestEnv;

    /// Holds the env lock with the AWS ambient vars cleared: the baseline for
    /// resolution tests whose `env=None` inputs read the process env (the
    /// oracle's env-mutating tests run in parallel).
    fn cleared_aws_env() -> TestEnv {
        TestEnv::apply(
            &[],
            &[
                "AWS_PROFILE",
                "AWS_REGION",
                "AWS_DEFAULT_REGION",
                "AWS_ACCESS_KEY_ID",
                "AWS_SECRET_ACCESS_KEY",
                "AWS_SESSION_TOKEN",
                "AWS_BEARER_TOKEN_BEDROCK",
                "AWS_BEDROCK_SKIP_AUTH",
            ],
        )
    }

    fn ambient_keys() -> Vec<(&'static str, &'static str)> {
        vec![
            ("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE"),
            ("AWS_SECRET_ACCESS_KEY", "secretexample"),
        ]
    }

    /// Oracle: "prefers explicit and scoped profiles over ambient AWS access
    /// keys" — a scoped `AWS_PROFILE` (the port's `options.profile` surface)
    /// records the profile and suppresses the ambient static keys.
    #[test]
    fn scoped_profile_suppresses_ambient_aws_access_keys() {
        let _env = TestEnv::apply(
            &ambient_keys(),
            &["AWS_PROFILE", "AWS_REGION", "AWS_DEFAULT_REGION"],
        );
        let scoped = env_map(&[("AWS_PROFILE", "scoped-profile")]);
        let resolved = resolve_endpoint_config(&claude_sonnet_4_5(), Some(&scoped), false, None);
        assert_eq!(resolved.profile.as_deref(), Some("scoped-profile"));
        assert_eq!(resolved.credentials, None);
    }

    /// Oracle: "uses ambient AWS access keys when no profile is configured".
    #[test]
    fn ambient_aws_access_keys_sign_without_a_profile() {
        let _env = TestEnv::apply(
            &ambient_keys(),
            &["AWS_PROFILE", "AWS_REGION", "AWS_DEFAULT_REGION"],
        );
        let resolved = resolve_endpoint_config(&claude_sonnet_4_5(), None, false, None);
        assert_eq!(resolved.profile, None);
        assert_eq!(
            resolved.credentials,
            Some(AwsCredentials {
                access_key_id: "AKIAEXAMPLE".to_string(),
                secret_access_key: "secretexample".to_string(),
                session_token: None,
            })
        );
    }

    /// Oracle: "uses ambient AWS access keys when only an ambient profile is
    /// set" — the ambient profile names `config.profile` but does NOT
    /// suppress the ambient static keys (upstream's `!optionsProfile` guard
    /// is scoped-env only).
    #[test]
    fn ambient_profile_keeps_the_ambient_aws_access_keys() {
        let mut settings = ambient_keys();
        settings.push(("AWS_PROFILE", "ambient-profile"));
        let _env = TestEnv::apply(&settings, &["AWS_REGION", "AWS_DEFAULT_REGION"]);
        let resolved = resolve_endpoint_config(&claude_sonnet_4_5(), None, true, None);
        assert_eq!(resolved.profile.as_deref(), Some("ambient-profile"));
        assert_eq!(
            resolved.credentials,
            Some(AwsCredentials {
                access_key_id: "AKIAEXAMPLE".to_string(),
                secret_access_key: "secretexample".to_string(),
                session_token: None,
            })
        );
    }

    /// Oracle: ambient `AWS_SESSION_TOKEN` rides the static-key chain.
    #[test]
    fn ambient_session_token_rides_the_static_keys() {
        let mut settings = ambient_keys();
        settings.push(("AWS_SESSION_TOKEN", "token"));
        let _env = TestEnv::apply(
            &settings,
            &["AWS_PROFILE", "AWS_REGION", "AWS_DEFAULT_REGION"],
        );
        let resolved = resolve_endpoint_config(&claude_sonnet_4_5(), None, false, None);
        assert_eq!(
            resolved.credentials,
            Some(AwsCredentials {
                access_key_id: "AKIAEXAMPLE".to_string(),
                secret_access_key: "secretexample".to_string(),
                session_token: Some("token".to_string()),
            })
        );
    }

    #[test]
    fn resolution_extracts_region_from_arn_regardless_of_env() {
        let env = env_map(&[("AWS_REGION", "us-east-1")]);
        let mut model = claude_sonnet_4_5();
        model.id = "arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/abc123"
            .to_string();
        let resolved = resolve_endpoint_config(&model, Some(&env), false, None);
        assert_eq!(resolved.region.as_deref(), Some("us-west-2"));

        model.id =
            "arn:aws-us-gov:bedrock:us-gov-west-1:123456789012:application-inference-profile/abc"
                .to_string();
        let resolved = resolve_endpoint_config(&model, Some(&env), false, None);
        assert_eq!(resolved.region.as_deref(), Some("us-gov-west-1"));
    }

    #[test]
    fn resolution_skip_auth_dummy_credentials_and_bearer() {
        let _env = cleared_aws_env();
        let env = env_map(&[("AWS_BEDROCK_SKIP_AUTH", "1")]);
        let resolved = resolve_endpoint_config(&claude_sonnet_4_5(), Some(&env), false, None);
        assert!(resolved.skip_auth);
        assert_eq!(resolved.credentials, Some(dummy_credentials()));
        // skipAuth wins over a bearer key.
        let resolved =
            resolve_endpoint_config(&claude_sonnet_4_5(), Some(&env), false, Some("key"));
        assert!(!resolved.use_bearer);

        // The generic API key (options.apiKey with its provider-config
        // fallback) is a Bedrock bearer token.
        let resolved =
            resolve_endpoint_config(&claude_sonnet_4_5(), None, false, Some("bedrock-api-key"));
        assert_eq!(resolved.bearer_token.as_deref(), Some("bedrock-api-key"));
        assert!(resolved.use_bearer);
        assert_eq!(resolved.credentials, None);

        let env = env_map(&[("AWS_BEARER_TOKEN_BEDROCK", "env-token")]);
        let resolved = resolve_endpoint_config(&claude_sonnet_4_5(), Some(&env), false, None);
        assert_eq!(resolved.bearer_token.as_deref(), Some("env-token"));
        assert!(resolved.use_bearer);
    }

    #[test]
    fn resolution_env_credentials_with_session_token() {
        let env = env_map(&[
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "SECRET"),
            ("AWS_SESSION_TOKEN", "TOKEN"),
        ]);
        let resolved = resolve_endpoint_config(&claude_sonnet_4_5(), Some(&env), false, None);
        assert_eq!(
            resolved.credentials,
            Some(AwsCredentials {
                access_key_id: "AKID".to_string(),
                secret_access_key: "SECRET".to_string(),
                session_token: Some("TOKEN".to_string()),
            })
        );
    }

    #[test]
    fn stream_url_keeps_arn_path_segments_and_encodes_colons() {
        let resolved = ResolvedEndpointConfig {
            region: Some("us-west-2".to_string()),
            ..ResolvedEndpointConfig::default()
        };
        assert_eq!(
            build_stream_url(
                &resolved,
                "arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/abc123"
            )
            .unwrap(),
            "https://bedrock-runtime.us-west-2.amazonaws.com/model/arn%3Aaws%3Abedrock%3Aus-west-2%3A123456789012%3Aapplication-inference-profile/abc123/stream"
        );
        // Custom endpoints keep their base URL; trailing slashes are trimmed.
        let resolved = ResolvedEndpointConfig {
            endpoint: Some("https://bedrock-vpc.example.com/".to_string()),
            region: Some("us-west-2".to_string()),
            ..ResolvedEndpointConfig::default()
        };
        assert_eq!(
            build_stream_url(&resolved, "my.model").unwrap(),
            "https://bedrock-vpc.example.com/model/my.model/stream"
        );
        // SDK resolver behavior: cn-* regions resolve under the .cn domain.
        let resolved = ResolvedEndpointConfig {
            region: Some("cn-north-1".to_string()),
            ..ResolvedEndpointConfig::default()
        };
        assert_eq!(
            build_stream_url(&resolved, "my.model").unwrap(),
            "https://bedrock-runtime.cn-north-1.amazonaws.com.cn/model/my.model/stream"
        );
        // No region anywhere: the profile chain would own it (M2d).
        assert!(build_stream_url(&ResolvedEndpointConfig::default(), "m").is_err());
    }

    #[tokio::test]
    async fn wire_request_hits_model_path_of_custom_endpoint() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("hi", "end_turn")).await;
        let mut model = claude_sonnet_4_5();
        model.base_url = server.uri();
        let env = signing_env("us-east-1");
        let (events, request) = capture_stream(
            &server,
            &model,
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        assert!(
            request
                .url
                .to_string()
                .ends_with("/model/us.anthropic.claude-sonnet-4-5-20250929-v1%3A0/stream"),
            "{}",
            request.url
        );
        assert_eq!(header_value(&request, "content-type"), "application/json");
        // SigV4 over the request: algorithm, credential scope with the region
        // and service name, and the signed date.
        let authorization = header_value(&request, "authorization");
        assert!(
            authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20"),
            "{authorization}"
        );
        assert!(
            authorization.contains("/us-east-1/bedrock/aws4_request"),
            "{authorization}"
        );
        assert!(
            authorization.contains("SignedHeaders=content-type;host;x-amz-date"),
            "{authorization}"
        );
        assert!(!header_value(&request, "x-amz-date").is_empty());
        done_of(&events);
    }

    // =========================================================================
    // Stream decoding: full happy path
    // =========================================================================

    #[tokio::test]
    async fn full_stream_decodes_events_and_builds_message() {
        let server = wiremock::MockServer::start().await;
        let redacted_base64 = "cnNuXzVaVnJpZjRKMGJYSXFtV2RsZWRqN1FJRmVOaWtSUWJF";
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                event_frame(
                    "contentBlockStart",
                    &json!({
                        "contentBlockIndex": 1,
                        "start": { "toolUse": { "toolUseId": "tool-1", "name": "edit" } }
                    }),
                ),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 1, "delta": { "toolUse": { "input": "{\"path\":" } } }),
                ),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 1, "delta": { "toolUse": { "input": "\"/tmp/x\"}" } } }),
                ),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 0, "delta": { "text": "Hel" } }),
                ),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 0, "delta": { "text": "lo" } }),
                ),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 2, "delta": { "reasoningContent": {
                        "text": "because", "signature": "sig" } } }),
                ),
                event_frame("contentBlockStop", &json!({ "contentBlockIndex": 0 })),
                event_frame("contentBlockStop", &json!({ "contentBlockIndex": 1 })),
                event_frame("contentBlockStop", &json!({ "contentBlockIndex": 2 })),
                event_frame(
                    "metadata",
                    &json!({
                        "usage": {
                            "inputTokens": 100,
                            "outputTokens": 5,
                            "totalTokens": 105,
                            "cacheReadInputTokens": 10,
                            "cacheWriteInputTokens": 20
                        }
                    }),
                ),
                event_frame("messageStop", &json!({ "stopReason": "tool_use" })),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;

        assert_eq!(
            event_types(&events),
            vec![
                "start",
                "toolcall_start",
                "toolcall_delta",
                "toolcall_delta",
                "text_start",
                "text_delta",
                "text_delta",
                "thinking_start",
                "thinking_delta",
                "text_end",
                "toolcall_end",
                "thinking_end",
                "done",
            ]
        );
        let (reason, message) = done_of(&events);
        assert_eq!(reason, SuccessReason::ToolUse);
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("tool_use"));
        // Blocks are stored in first-appearance order: the toolUse start
        // arrives before the text deltas, matching upstream's block creation.
        assert_eq!(
            message.content,
            vec![
                AssistantBlock::ToolCall(ToolCall {
                    id: "tool-1".to_string(),
                    name: "edit".to_string(),
                    arguments: json!({ "path": "/tmp/x" }),
                    thought_signature: None,
                    namespace: None,
                }),
                text_block("Hello"),
                thinking_block("because", Some("sig"), false),
            ]
        );
        assert_eq!(message.usage.input, 100);
        assert_eq!(message.usage.output, 5);
        assert_eq!(message.usage.cache_read, 10);
        assert_eq!(message.usage.cache_write, 20);
        assert_eq!(message.usage.total_tokens, 105);
        // Cost recomputed from the model rates.
        assert_eq!(message.usage.cost.input, (3.0 / 1_000_000.0) * 100.0);
        assert_eq!(message.usage.cost.output, (15.0 / 1_000_000.0) * 5.0);
        // The event sequence must satisfy the canonical frame reducer.
        apply_all(&events);
        let _ = redacted_base64;
    }

    #[tokio::test]
    async fn streamed_tool_arguments_preserve_empty_property_names() {
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                event_frame(
                    "contentBlockStart",
                    &json!({
                        "contentBlockIndex": 0,
                        "start": { "toolUse": { "toolUseId": "tool-1", "name": "edit" } }
                    }),
                ),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 0, "delta": { "toolUse": { "input":
                        "{\"path\":\"/workspace/foobar/file.js\",\"edits\":[{\"oldText\":\"first\",\"newText\":\"updated first\"},{\"oldText\":\"second\",\"newText\":\"updated second\",\"\":\"\"}]}" } } }),
                ),
                event_frame("contentBlockStop", &json!({ "contentBlockIndex": 0 })),
                event_frame("messageStop", &json!({ "stopReason": "tool_use" })),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("Use the tool"),
            &stream_options(env),
        )
        .await;
        let (_, message) = done_of(&events);
        assert_eq!(
            message.content[0],
            AssistantBlock::ToolCall(ToolCall {
                id: "tool-1".to_string(),
                name: "edit".to_string(),
                arguments: json!({
                    "path": "/workspace/foobar/file.js",
                    "edits": [
                        { "oldText": "first", "newText": "updated first" },
                        { "oldText": "second", "newText": "updated second", "": "" },
                    ],
                }),
                thought_signature: None,
                namespace: None,
            })
        );
    }

    #[tokio::test]
    async fn non_tool_block_start_is_ignored_and_deltas_still_flow() {
        // Upstream: contentBlockStart without toolUse opens nothing; a later
        // text delta lazily creates the text block.
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                event_frame(
                    "contentBlockStart",
                    &json!({ "contentBlockIndex": 0, "start": {} }),
                ),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 0, "delta": { "text": "hi" } }),
                ),
                event_frame("contentBlockStop", &json!({ "contentBlockIndex": 0 })),
                event_frame("messageStop", &json!({ "stopReason": "end_turn" })),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        assert_eq!(
            event_types(&events),
            vec!["start", "text_start", "text_delta", "text_end", "done"]
        );
    }

    #[tokio::test]
    async fn user_role_message_start_fails_the_stream() {
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[event_frame("messageStart", &json!({ "role": "user" }))],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Unexpected assistant message start but got user message start instead")
        );
    }

    // =========================================================================
    // Stop reasons (raw-stop-reason oracle)
    // =========================================================================

    #[tokio::test]
    async fn raw_stop_reason_preserved_for_successful_stops() {
        for (wire, stop_reason) in [
            ("end_turn", StopReason::Stop),
            ("stop_sequence", StopReason::Stop),
            ("max_tokens", StopReason::Length),
            ("model_context_window_exceeded", StopReason::Length),
        ] {
            let server = wiremock::MockServer::start().await;
            serve_frames(&server, &text_stream_frames("hi", wire)).await;
            let env = signing_env("us-east-1");
            let (events, _) = capture_stream(
                &server,
                &claude_sonnet_4_5(),
                &user_context("hi"),
                &stream_options(env),
            )
            .await;
            let (reason, message) = done_of(&events);
            assert_eq!(message.stop_reason, stop_reason, "{wire}");
            assert_eq!(message.raw_stop_reason.as_deref(), Some(wire));
            assert_eq!(message.error_message, None);
            assert_eq!(
                reason,
                match stop_reason {
                    StopReason::Length => SuccessReason::Length,
                    _ => SuccessReason::Stop,
                }
            );
            assert!(message.diagnostics.is_none());
        }
    }

    #[tokio::test]
    async fn raw_stop_reason_preserved_for_tool_use() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("hi", "tool_use")).await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hi"),
            &stream_options(env),
        )
        .await;
        let (reason, message) = done_of(&events);
        assert_eq!(reason, SuccessReason::ToolUse);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("tool_use"));
    }

    #[tokio::test]
    async fn provider_error_stop_reason_becomes_error_event() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("hi", "guardrail_intervened")).await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hi"),
            &stream_options(env),
        )
        .await;
        let message = error_of(&events);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.raw_stop_reason.as_deref(),
            Some("guardrail_intervened")
        );
        assert_eq!(
            message.error_message.as_deref(),
            Some("Provider stopped with: guardrail_intervened")
        );
        // The failure diagnostic carries the response request id (the guard
        // throw reaches the catch like upstream, with the fallback id).
        assert_eq!(diagnostic_of(&message), &json!({ "requestId": "req-123" }));
    }

    #[tokio::test]
    async fn missing_stop_reason_fails_the_stream() {
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                event_frame("messageStop", &json!({})),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hi"),
            &stream_options(env),
        )
        .await;
        let message = error_of(&events);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(message.raw_stop_reason, None);
        // The missing stop reason maps to a bare error, which the guard
        // throws as the fallback message (upstream lines 337-339).
        assert_eq!(
            message.error_message.as_deref(),
            Some("An unknown error occurred")
        );
    }

    #[tokio::test]
    async fn empty_event_stream_fails_without_stop_reason() {
        let server = wiremock::MockServer::start().await;
        serve_bytes(&server, 200, Vec::new()).await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hi"),
            &stream_options(env),
        )
        .await;
        let message = error_of(&events);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Bedrock stream ended without a stop reason")
        );
    }

    // =========================================================================
    // 1h cache-write cost (cache-write-1h-cost oracle)
    // =========================================================================

    #[tokio::test]
    async fn one_hour_cache_details_price_at_twice_the_input_rate() {
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                event_frame(
                    "metadata",
                    &json!({
                        "usage": {
                            "inputTokens": 100,
                            "outputTokens": 5,
                            "totalTokens": 1_000_105,
                            "cacheWriteInputTokens": 1_000_000,
                            "cacheDetails": [
                                { "ttl": "1h", "inputTokens": 150_000 },
                                { "ttl": "5m", "inputTokens": 600_000 },
                                { "ttl": "1h", "inputTokens": 250_000 },
                            ]
                        }
                    }),
                ),
                event_frame("messageStop", &json!({ "stopReason": "end_turn" })),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hi"),
            &stream_options(env),
        )
        .await;
        let (_, message) = done_of(&events);
        // The total cache write is preserved and the 1h subset split out.
        assert_eq!(message.usage.cache_write, 1_000_000);
        assert_eq!(message.usage.cache_write_1h, Some(400_000));
        let expected = (600_000.0 * 3.75 + 400_000.0 * 3.0 * 2.0) / 1_000_000.0;
        assert!(
            (message.usage.cost.cache_write - expected).abs() < 1e-12,
            "{:?}",
            message.usage.cost
        );
        assert_eq!(message.usage.total_tokens, 1_000_105);
    }

    // =========================================================================
    // Redacted reasoning (redacted-reasoning oracle)
    // =========================================================================

    const REDACTED_BASE64: &str = "cnNuXzVaVnJpZjRKMGJYSXFtV2RsZWRqN1FJRmVOaWtSUWJF";

    fn gpt_model() -> Model {
        model("global.openai.gpt-5.6-terra", "GPT-5.6 Terra (Global)")
    }

    fn redacted_frames() -> Vec<Vec<u8>> {
        vec![
            event_frame("messageStart", &json!({ "role": "assistant" })),
            event_frame(
                "contentBlockDelta",
                &json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": {
                    "redactedContent": REDACTED_BASE64 } } }),
            ),
            event_frame("contentBlockStop", &json!({ "contentBlockIndex": 0 })),
            event_frame(
                "contentBlockDelta",
                &json!({ "contentBlockIndex": 1, "delta": { "text": "done" } }),
            ),
            event_frame("contentBlockStop", &json!({ "contentBlockIndex": 1 })),
            event_frame("messageStop", &json!({ "stopReason": "end_turn" })),
        ]
    }

    #[tokio::test]
    async fn redacted_reasoning_becomes_opaque_thinking_block() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &redacted_frames()).await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &gpt_model(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        let (_, message) = done_of(&events);
        assert_eq!(message.stop_reason, StopReason::Stop);
        // Reasoning precedes the answer, matching the order Bedrock streamed.
        let kinds: Vec<&str> = message
            .content
            .iter()
            .map(|block| match block {
                AssistantBlock::Text(_) => "text",
                AssistantBlock::Thinking(_) => "thinking",
                AssistantBlock::ToolCall(_) => "toolCall",
            })
            .collect();
        assert_eq!(kinds, vec!["thinking", "text"]);
        assert_eq!(message.content[1], text_block("done"));
        let AssistantBlock::Thinking(thinking) = &message.content[0] else {
            panic!("expected thinking block");
        };
        // Same representation Anthropic redacted thinking uses: the opaque
        // payload rides `thinkingSignature` with `redacted: true`.
        assert_eq!(thinking.redacted, Some(true));
        assert_eq!(thinking.thinking, REDACTED_THINKING_PLACEHOLDER);
        assert_eq!(
            thinking.thinking_signature.as_deref(),
            Some(REDACTED_BASE64)
        );
    }

    #[tokio::test]
    async fn redacted_reasoning_encoded_without_content_block_stop() {
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": {
                        "redactedContent": REDACTED_BASE64 } } }),
                ),
                event_frame("messageStop", &json!({ "stopReason": "end_turn" })),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &gpt_model(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        let (_, message) = done_of(&events);
        let AssistantBlock::Thinking(thinking) = &message.content[0] else {
            panic!("expected thinking block");
        };
        assert_eq!(
            thinking.thinking_signature.as_deref(),
            Some(REDACTED_BASE64)
        );
    }

    #[tokio::test]
    async fn redacted_reasoning_joins_across_deltas_once() {
        // Split the payload: the first chunk must decode as strict base64, so
        // split at a 4-byte boundary ("cnNu" / "_zVa...").
        let (head, tail) = REDACTED_BASE64.split_at(8);
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": {
                        "redactedContent": head } } }),
                ),
                event_frame(
                    "contentBlockDelta",
                    &json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": {
                        "redactedContent": tail } } }),
                ),
                event_frame("contentBlockStop", &json!({ "contentBlockIndex": 0 })),
                event_frame("messageStop", &json!({ "stopReason": "end_turn" })),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &gpt_model(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        let (_, message) = done_of(&events);
        let AssistantBlock::Thinking(thinking) = &message.content[0] else {
            panic!("expected thinking block");
        };
        assert_eq!(
            thinking.thinking_signature.as_deref(),
            Some(REDACTED_BASE64)
        );
        // The placeholder marks the block once, not once per delta.
        assert_eq!(thinking.thinking, REDACTED_THINKING_PLACEHOLDER);
        // Only one thinking_delta carries the placeholder.
        let placeholder_deltas = events
            .iter()
            .filter(|event| matches!(event, AssistantMessageEvent::ThinkingDelta { delta, .. } if delta == REDACTED_THINKING_PLACEHOLDER))
            .count();
        assert_eq!(placeholder_deltas, 1);
    }

    #[tokio::test]
    async fn redacted_reasoning_replays_as_redacted_content() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![
                Message::User(UserMessage {
                    content: StringOrBlocks::Text("hello".to_string()),
                    timestamp: TS,
                }),
                assistant_message(
                    &gpt_model(),
                    vec![
                        thinking_block("", Some(REDACTED_BASE64), true),
                        text_block("done"),
                    ],
                    StopReason::Stop,
                ),
                Message::User(UserMessage {
                    content: StringOrBlocks::Text("continue".to_string()),
                    timestamp: TS + 1,
                }),
            ],
            tools: None,
        });
        let env = signing_env("us-east-1");
        let (_, request) = capture_stream(&server, &gpt_model(), &ctx, &stream_options(env)).await;
        let body = request_body(&request);
        let assistant = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == json!("assistant"))
            .unwrap();
        assert_eq!(
            assistant["content"],
            json!([
                { "reasoningContent": { "redactedContent": REDACTED_BASE64 } },
                { "text": "done" }
            ])
        );
    }

    #[tokio::test]
    async fn redacted_reasoning_replays_before_its_tool_use_block() {
        // Bedrock rejects a tool continuation whose reasoning block is
        // missing or reordered, so the payload must land ahead of the
        // matching toolUse.
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![
                Message::User(UserMessage {
                    content: StringOrBlocks::Text("read the file".to_string()),
                    timestamp: TS,
                }),
                assistant_message(
                    &gpt_model(),
                    vec![
                        thinking_block("", Some(REDACTED_BASE64), true),
                        AssistantBlock::ToolCall(ToolCall {
                            id: "tool-1".to_string(),
                            name: "read".to_string(),
                            arguments: json!({ "path": "/tmp/a.txt" }),
                            thought_signature: None,
                            namespace: None,
                        }),
                    ],
                    StopReason::ToolUse,
                ),
                Message::ToolResult(ToolResultMessage {
                    tool_call_id: "tool-1".to_string(),
                    tool_name: "read".to_string(),
                    content: vec![TextOrImageBlock::Text(TextContent {
                        text: "file body".to_string(),
                        text_signature: None,
                    })],
                    details: None,
                    usage: None,
                    is_error: false,
                    timestamp: TS + 1,
                }),
            ],
            tools: None,
        });
        let env = signing_env("us-east-1");
        let (_, request) = capture_stream(&server, &gpt_model(), &ctx, &stream_options(env)).await;
        let body = request_body(&request);
        assert_eq!(
            assistant_content(&body),
            json!([
                { "reasoningContent": { "redactedContent": REDACTED_BASE64 } },
                { "toolUse": { "toolUseId": "tool-1", "name": "read", "input": { "path": "/tmp/a.txt" } } }
            ])
        );
    }

    #[tokio::test]
    async fn signed_thinking_replays_as_reasoning_text_for_claude_only() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;

        // Claude: signed thinking replays with its signature.
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![
                Message::User(UserMessage {
                    content: StringOrBlocks::Text("hi".to_string()),
                    timestamp: TS,
                }),
                assistant_message(
                    &claude_sonnet_4_5(),
                    vec![thinking_block("deep thought", Some("sig-1"), false)],
                    StopReason::Stop,
                ),
            ],
            tools: None,
        });
        let env = signing_env("us-east-1");
        let (_, request) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &ctx,
            &stream_options(env.clone()),
        )
        .await;
        let body = request_body(&request);
        assert_eq!(
            assistant_content(&body),
            json!([
                { "reasoningContent": { "reasoningText": { "text": "deep thought", "signature": "sig-1" } } }
            ])
        );

        // Claude: unsigned thinking falls back to plain text (Bedrock rejects
        // signature-less reasoning replay).
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![
                Message::User(UserMessage {
                    content: StringOrBlocks::Text("hi".to_string()),
                    timestamp: TS,
                }),
                assistant_message(
                    &claude_sonnet_4_5(),
                    vec![thinking_block("deep thought", None, false)],
                    StopReason::Stop,
                ),
            ],
            tools: None,
        });
        let (_, request) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &ctx,
            &stream_options(env.clone()),
        )
        .await;
        let body = request_body(&request);
        assert_eq!(
            assistant_content(&body),
            json!([{ "text": "deep thought" }])
        );

        // Non-Claude: thinking replays without the signature member.
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![
                Message::User(UserMessage {
                    content: StringOrBlocks::Text("hi".to_string()),
                    timestamp: TS,
                }),
                assistant_message(
                    &gpt_model(),
                    vec![thinking_block("deep thought", Some("sig-1"), false)],
                    StopReason::Stop,
                ),
            ],
            tools: None,
        });
        let (_, request) = capture_stream(&server, &gpt_model(), &ctx, &stream_options(env)).await;
        let body = request_body(&request);
        assert_eq!(
            assistant_content(&body),
            json!([
                { "reasoningContent": { "reasoningText": { "text": "deep thought" } } }
            ])
        );
    }

    // =========================================================================
    // Message conversion (convert-messages oracle)
    // =========================================================================

    async fn capture_body_with_context(model: &Model, ctx: &TranscriptContext) -> Value {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let (_, request) = capture_stream(
            &server,
            model,
            ctx,
            &stream_options(signing_env("us-east-1")),
        )
        .await;
        request_body(&request)
    }

    /// Same as [`capture_body_with_context`] but with the default (short)
    /// cache retention, for the cache-point tests.
    async fn capture_body_short_retention(model: &Model, ctx: &TranscriptContext) -> Value {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let (_, request) = capture_stream(
            &server,
            model,
            ctx,
            &short_retention_options(signing_env("us-east-1")),
        )
        .await;
        request_body(&request)
    }

    #[tokio::test]
    async fn blank_user_content_becomes_placeholder() {
        let body = capture_body_with_context(
            &claude_sonnet_4_5(),
            &normalize_context(&Context {
                system_prompt: None,
                messages: vec![Message::User(UserMessage {
                    content: StringOrBlocks::Text("   ".to_string()),
                    timestamp: TS,
                })],
                tools: None,
            }),
        )
        .await;
        assert_eq!(
            body["messages"],
            json!([{ "role": "user", "content": [{ "text": "<empty>" }] }])
        );
    }

    #[tokio::test]
    async fn blank_user_text_blocks_filtered_when_others_remain() {
        let body = capture_body_with_context(
            &claude_sonnet_4_5(),
            &normalize_context(&Context {
                system_prompt: None,
                messages: vec![Message::User(UserMessage {
                    content: StringOrBlocks::Blocks(vec![
                        TextOrImageBlock::Text(TextContent {
                            text: String::new(),
                            text_signature: None,
                        }),
                        TextOrImageBlock::Text(TextContent {
                            text: "hello".to_string(),
                            text_signature: None,
                        }),
                    ]),
                    timestamp: TS,
                })],
                tools: None,
            }),
        )
        .await;
        assert_eq!(body["messages"][0]["content"], json!([{ "text": "hello" }]));
    }

    #[tokio::test]
    async fn assistant_blocks_emptied_are_skipped_and_empty_assistants_dropped() {
        // Whitespace-only assistant text is dropped; the message has no
        // remaining blocks, so the whole message is skipped.
        let body = capture_body_with_context(
            &claude_sonnet_4_5(),
            &normalize_context(&Context {
                system_prompt: None,
                messages: vec![
                    Message::User(UserMessage {
                        content: StringOrBlocks::Text("hi".to_string()),
                        timestamp: TS,
                    }),
                    assistant_message(
                        &claude_sonnet_4_5(),
                        vec![text_block("   ")],
                        StopReason::Stop,
                    ),
                ],
                tools: None,
            }),
        )
        .await;
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], json!("user"));
    }

    #[tokio::test]
    async fn blank_tool_result_content_becomes_placeholder() {
        let body = capture_body_with_context(
            &claude_sonnet_4_5(),
            &normalize_context(&Context {
                system_prompt: None,
                messages: vec![Message::ToolResult(ToolResultMessage {
                    tool_call_id: "tool-1".to_string(),
                    tool_name: "tool".to_string(),
                    content: vec![TextOrImageBlock::Text(TextContent {
                        text: String::new(),
                        text_signature: None,
                    })],
                    details: None,
                    usage: None,
                    is_error: false,
                    timestamp: TS,
                })],
                tools: None,
            }),
        )
        .await;
        let content = &body["messages"][0]["content"][0]["toolResult"]["content"];
        assert_eq!(content, &json!([{ "text": "<empty>" }]));
    }

    #[tokio::test]
    async fn consecutive_tool_results_merge_into_one_user_message() {
        let body = capture_body_with_context(
            &claude_sonnet_4_5(),
            &normalize_context(&Context {
                system_prompt: None,
                messages: vec![
                    Message::User(UserMessage {
                        content: StringOrBlocks::Text("go".to_string()),
                        timestamp: TS,
                    }),
                    assistant_message(
                        &claude_sonnet_4_5(),
                        vec![AssistantBlock::ToolCall(ToolCall {
                            id: "tool-1".to_string(),
                            name: "a".to_string(),
                            arguments: json!({ "k": "v", "": "dropped" }),
                            thought_signature: None,
                            namespace: None,
                        })],
                        StopReason::ToolUse,
                    ),
                    Message::ToolResult(ToolResultMessage {
                        tool_call_id: "tool-1".to_string(),
                        tool_name: "a".to_string(),
                        content: vec![TextOrImageBlock::Text(TextContent {
                            text: "one".to_string(),
                            text_signature: None,
                        })],
                        details: None,
                        usage: None,
                        is_error: false,
                        timestamp: TS + 1,
                    }),
                    Message::ToolResult(ToolResultMessage {
                        tool_call_id: "tool-2".to_string(),
                        tool_name: "b".to_string(),
                        content: vec![TextOrImageBlock::Text(TextContent {
                            text: "two".to_string(),
                            text_signature: None,
                        })],
                        details: None,
                        usage: None,
                        is_error: true,
                        timestamp: TS + 2,
                    }),
                ],
                tools: None,
            }),
        )
        .await;
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        // Empty-key arguments are stripped only from the replayed input.
        assert_eq!(
            messages[1]["content"],
            json!([{ "toolUse": { "toolUseId": "tool-1", "name": "a", "input": { "k": "v" } } }])
        );
        assert_eq!(
            messages[2],
            json!({
                "role": "user",
                "content": [
                    { "toolResult": { "toolUseId": "tool-1", "content": [{ "text": "one" }], "status": "success" } },
                    { "toolResult": { "toolUseId": "tool-2", "content": [{ "text": "two" }], "status": "error" } },
                ]
            })
        );
    }

    #[tokio::test]
    async fn user_images_map_to_bedrock_image_blocks() {
        let body = capture_body_with_context(
            &claude_sonnet_4_5(),
            &normalize_context(&Context {
                system_prompt: None,
                messages: vec![Message::User(UserMessage {
                    content: StringOrBlocks::Blocks(vec![
                        TextOrImageBlock::Image(crate::ai::types::ImageContent {
                            data: "aGk=".to_string(),
                            mime_type: "image/png".to_string(),
                        }),
                        TextOrImageBlock::Text(TextContent {
                            text: "what is this?".to_string(),
                            text_signature: None,
                        }),
                    ]),
                    timestamp: TS,
                })],
                tools: None,
            }),
        )
        .await;
        assert_eq!(
            body["messages"][0]["content"],
            json!([
                { "image": { "source": { "bytes": "aGk=" }, "format": "png" } },
                { "text": "what is this?" }
            ])
        );
    }

    #[tokio::test]
    async fn unknown_image_mime_type_fails_the_stream() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Image(
                    crate::ai::types::ImageContent {
                        data: "aGk=".to_string(),
                        mime_type: "image/tiff".to_string(),
                    },
                )]),
                timestamp: TS,
            })],
            tools: None,
        });
        let env = signing_env("us-east-1");
        let mut model = claude_sonnet_4_5();
        model.base_url = server.uri();
        let mut rx = BedrockConverseStream.stream(&cfg(), &model, &ctx, &stream_options(env));
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let message = error_of(&events);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Unknown image type: image/tiff")
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn tool_result_images_map_to_bedrock_image_blocks() {
        let body = capture_body_with_context(
            &claude_sonnet_4_5(),
            &normalize_context(&Context {
                system_prompt: None,
                messages: vec![Message::ToolResult(ToolResultMessage {
                    tool_call_id: "tool-1".to_string(),
                    tool_name: "screenshot".to_string(),
                    content: vec![TextOrImageBlock::Image(crate::ai::types::ImageContent {
                        data: "aGk=".to_string(),
                        mime_type: "image/jpeg".to_string(),
                    })],
                    details: None,
                    usage: None,
                    is_error: false,
                    timestamp: TS,
                })],
                tools: None,
            }),
        )
        .await;
        assert_eq!(
            body["messages"][0]["content"][0]["toolResult"]["content"],
            json!([{ "image": { "source": { "bytes": "aGk=" }, "format": "jpeg" } }])
        );
    }

    // =========================================================================
    // Cache points (thinking-payload oracle, cache-point test)
    // =========================================================================

    #[tokio::test]
    async fn cache_points_injected_for_claude_models() {
        let body = capture_body_short_retention(
            &claude_sonnet_4_5(),
            &normalize_context(&Context {
                system_prompt: Some("You are helpful.".to_string()),
                messages: vec![Message::User(UserMessage {
                    content: StringOrBlocks::Text("Hello".to_string()),
                    timestamp: TS,
                })],
                tools: None,
            }),
        )
        .await;
        // Default (short) retention: a default cache point on the system and
        // the last user message, no TTL.
        assert_eq!(body["system"].as_array().unwrap().len(), 2);
        assert_eq!(
            body["system"][1],
            json!({ "cachePoint": { "type": "default" } })
        );
        let last = body["messages"].as_array().unwrap().last().unwrap();
        let last_content = last["content"].as_array().unwrap().last().unwrap();
        assert_eq!(
            last_content,
            &json!({ "cachePoint": { "type": "default" } })
        );
    }

    #[tokio::test]
    async fn long_cache_retention_sets_one_hour_ttl() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let ctx = normalize_context(&Context {
            system_prompt: Some("You are helpful.".to_string()),
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("Hello".to_string()),
                timestamp: TS,
            })],
            tools: None,
        });
        let options = StreamOptions {
            cache_retention: Some(CacheRetention::Long),
            env: signing_env("us-east-1"),
            ..StreamOptions::default()
        };
        let (_, request) = capture_stream(&server, &claude_sonnet_4_5(), &ctx, &options).await;
        let body = request_body(&request);
        assert_eq!(
            body["system"][1],
            json!({ "cachePoint": { "type": "default", "ttl": "1h" } })
        );
        let last_content = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(
            last_content,
            &json!({ "cachePoint": { "type": "default", "ttl": "1h" } })
        );
    }

    #[tokio::test]
    async fn non_claude_models_get_no_cache_points() {
        let body = capture_body_with_context(
            &nova_lite(),
            &normalize_context(&Context {
                system_prompt: Some("You are helpful.".to_string()),
                messages: vec![Message::User(UserMessage {
                    content: StringOrBlocks::Text("Hello".to_string()),
                    timestamp: TS,
                })],
                tools: None,
            }),
        )
        .await;
        assert_eq!(body["system"].as_array().unwrap().len(), 1);
        let last_content = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_ne!(last_content.get("cachePoint"), Some(&json!({})));
        assert!(last_content.get("cachePoint").is_none());
    }

    #[tokio::test]
    async fn force_cache_env_enables_cache_points_for_non_claude() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let ctx = normalize_context(&Context {
            system_prompt: Some("You are helpful.".to_string()),
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("Hello".to_string()),
                timestamp: TS,
            })],
            tools: None,
        });
        let mut env = signing_env("us-east-1").unwrap();
        env.insert("AWS_BEDROCK_FORCE_CACHE".to_string(), "1".to_string());
        let options = StreamOptions {
            cache_retention: Some(CacheRetention::Short),
            env: Some(env),
            ..StreamOptions::default()
        };
        let (_, request) = capture_stream(&server, &nova_lite(), &ctx, &options).await;
        let body = request_body(&request);
        assert!(
            body["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()
                .get("cachePoint")
                .is_some()
        );
    }

    #[tokio::test]
    async fn inference_profile_name_identifies_claude_for_caching() {
        // Application inference profiles: the ARN lacks the model name, so
        // model.name decides.
        let mut profile_model = model(
            "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile",
            "Claude Sonnet 4.6",
        );
        profile_model.base_url = "https://unused.example.com".to_string();
        let body = capture_body_short_retention(
            &profile_model,
            &normalize_context(&Context {
                system_prompt: Some("You are helpful.".to_string()),
                messages: vec![Message::User(UserMessage {
                    content: StringOrBlocks::Text("Hello".to_string()),
                    timestamp: TS,
                })],
                tools: None,
            }),
        )
        .await;
        assert_eq!(body["system"].as_array().unwrap().len(), 2);
        let last_content = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(
            last_content,
            &json!({ "cachePoint": { "type": "default" } })
        );
    }

    // =========================================================================
    // Thinking payloads (thinking-payload oracle, asserted on the wire body)
    // =========================================================================

    async fn capture_additional_fields(
        model: &Model,
        reasoning: Option<ThinkingLevel>,
        env: Option<ProviderEnv>,
    ) -> (Value, Value) {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let ctx = normalize_context(&Context {
            system_prompt: Some("Hello".to_string()),
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("Hello".to_string()),
                timestamp: TS,
            })],
            tools: None,
        });
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                cache_retention: Some(CacheRetention::None),
                env: env.or_else(|| signing_env("us-east-1")),
                ..StreamOptions::default()
            },
            reasoning,
            ..SimpleStreamOptions::default()
        };
        let (_, request) = capture_simple(&server, model, &ctx, &options).await;
        let body = request_body(&request);
        (
            body["additionalModelRequestFields"].clone(),
            body["inferenceConfig"].clone(),
        )
    }

    #[tokio::test]
    async fn adaptive_thinking_for_claude_opus_4_8() {
        let model = model(
            "global.anthropic.claude-opus-4-8-v1",
            "Claude Opus 4.8 (Global)",
        );
        let (additional, _) =
            capture_additional_fields(&model, Some(ThinkingLevel::High), None).await;
        // v1.0.0: Opus 4.8 accepts thinking.block_binding, so replayed signed
        // thinking blocks are dropped on a prefix mismatch, and the binding
        // controls beta rides along.
        assert_eq!(
            additional["thinking"],
            json!({
                "type": "adaptive",
                "display": "summarized",
                "block_binding": { "prefix_mismatch_behavior": "drop_block" }
            })
        );
        assert_eq!(additional["output_config"], json!({ "effort": "high" }));
        assert_eq!(
            additional["anthropic_beta"],
            json!(["thinking-binding-controls-2026-08-01"])
        );
    }

    #[tokio::test]
    async fn xhigh_effort_mapping_for_adaptive_models() {
        for (id, name) in [
            (
                "global.anthropic.claude-opus-4-8-v1",
                "Claude Opus 4.8 (Global)",
            ),
            ("global.anthropic.claude-opus-5", "Claude Opus 5"),
            ("global.anthropic.claude-sonnet-5", "Claude Sonnet 5"),
            ("global.anthropic.claude-fable-5", "Claude Fable 5"),
        ] {
            let model = model(id, name);
            let (additional, _) =
                capture_additional_fields(&model, Some(ThinkingLevel::Xhigh), None).await;
            assert_eq!(
                additional["output_config"],
                json!({ "effort": "xhigh" }),
                "{id}"
            );
        }
    }

    #[tokio::test]
    async fn fixed_budget_thinking_for_older_claude_models() {
        let (additional, inference_config) =
            capture_additional_fields(&claude_sonnet_4_5(), Some(ThinkingLevel::High), None).await;
        assert_eq!(
            additional["thinking"],
            json!({ "type": "enabled", "budget_tokens": 16384, "display": "summarized" })
        );
        assert_eq!(
            additional["anthropic_beta"],
            json!(["interleaved-thinking-2025-05-14"])
        );
        // streamSimple raises the cap by the thinking budget and clamps to the
        // model cap (64000), then reserves an answer window for the budget.
        assert_eq!(inference_config["maxTokens"], json!(64000));

        // Low reasoning maps to the 2048 default budget.
        let (additional, _) =
            capture_additional_fields(&claude_sonnet_4_5(), Some(ThinkingLevel::Low), None).await;
        assert_eq!(additional["thinking"]["budget_tokens"], json!(2048));
    }

    #[tokio::test]
    async fn govcloud_model_ids_omit_display_and_add_beta() {
        let mut gov = model(
            "us-gov.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "Claude Sonnet 4.5 (GovCloud)",
        );
        gov.base_url = "https://bedrock-runtime.us-gov-west-1.amazonaws.com".to_string();
        let (additional, _) =
            capture_additional_fields(&gov, Some(ThinkingLevel::High), None).await;
        assert_eq!(
            additional["thinking"],
            json!({ "type": "enabled", "budget_tokens": 16384 })
        );
        assert_eq!(
            additional["anthropic_beta"],
            json!(["interleaved-thinking-2025-05-14"])
        );
    }

    #[tokio::test]
    async fn govcloud_regions_omit_display_on_adaptive_thinking() {
        let model = model(
            "global.anthropic.claude-opus-4-8-v1",
            "Claude Opus 4.8 (Global)",
        );
        let env = signing_env("us-gov-west-1");
        let (additional, _) =
            capture_additional_fields(&model, Some(ThinkingLevel::High), Some(env.unwrap())).await;
        assert_eq!(additional["thinking"], json!({ "type": "adaptive" }));
        assert_eq!(additional["output_config"], json!({ "effort": "high" }));
        assert!(additional.get("anthropic_beta").is_none());
    }

    #[tokio::test]
    async fn inference_profiles_resolve_thinking_via_model_name() {
        // Adaptive via the profile's display name.
        let adaptive_profile = model(
            "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile",
            "Claude Opus 4.6",
        );
        let (additional, _) =
            capture_additional_fields(&adaptive_profile, Some(ThinkingLevel::High), None).await;
        assert_eq!(
            additional["thinking"],
            json!({ "type": "adaptive", "display": "summarized" })
        );
        assert_eq!(additional["output_config"], json!({ "effort": "high" }));

        // Fixed budget via the profile's display name.
        let budget_profile = model(
            "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile",
            "Claude Sonnet 4.5",
        );
        let (additional, _) =
            capture_additional_fields(&budget_profile, Some(ThinkingLevel::High), None).await;
        assert_eq!(additional["thinking"]["type"], json!("enabled"));
        assert!(additional["thinking"]["budget_tokens"].is_u64());
        assert_eq!(
            additional["anthropic_beta"],
            json!(["interleaved-thinking-2025-05-14"])
        );
    }

    #[tokio::test]
    async fn thinking_level_map_overrides_effort() {
        let mut model = model("global.anthropic.claude-opus-4-8-v1", "Claude Opus 4.8");
        let mut map = std::collections::BTreeMap::new();
        map.insert("high".to_string(), Some("max".to_string()));
        model.thinking_level_map = Some(map);
        let (additional, _) =
            capture_additional_fields(&model, Some(ThinkingLevel::High), None).await;
        assert_eq!(additional["output_config"], json!({ "effort": "max" }));
    }

    #[tokio::test]
    async fn no_reasoning_omits_additional_fields_and_non_claude_gets_none() {
        // No reasoning requested.
        let (additional, _) = capture_additional_fields(&claude_sonnet_4_5(), None, None).await;
        assert!(additional.is_null());
        // Reasoning on a non-Claude model sends no additional fields.
        let (additional, _) =
            capture_additional_fields(&gpt_model(), Some(ThinkingLevel::High), None).await;
        assert!(additional.is_null());
    }

    // =========================================================================
    // Tools (convert-messages oracle strict-sampling test)
    // =========================================================================

    fn lookup_tool(strict: Strict) -> Tool {
        Tool {
            name: "lookup".to_string(),
            description: "Look up a value".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"]
            }),
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict,
            })),
        }
    }

    #[tokio::test]
    async fn strict_tool_gated_by_model_capability() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("Use the tool".to_string()),
                timestamp: TS,
            })],
            tools: Some(vec![lookup_tool(Strict::Require)]),
        });
        let env = signing_env("us-east-1");

        // Strict-capable model: native strict tool use.
        let (_, request) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &ctx,
            &stream_options(env.clone()),
        )
        .await;
        let tool_spec = &request_body(&request)["toolConfig"]["tools"][0]["toolSpec"];
        assert_eq!(tool_spec["strict"], json!(true));
        // The strict conversion: every object requires all properties and
        // forbids additional ones.
        let schema = &tool_spec["inputSchema"]["json"];
        assert_eq!(schema["required"], json!(["value"]));
        assert_eq!(schema["additionalProperties"], json!(false));

        // Non-strict model (no compat): a "prefer" request degrades to the
        // plain schema without the strict member.
        let prefer_ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("Use the tool".to_string()),
                timestamp: TS,
            })],
            tools: Some(vec![lookup_tool(Strict::Prefer)]),
        });
        let (_, request) =
            capture_stream(&server, &nova_lite(), &prefer_ctx, &stream_options(env)).await;
        let tool_spec = &request_body(&request)["toolConfig"]["tools"][0]["toolSpec"];
        assert!(tool_spec.get("strict").is_none());
        assert_eq!(
            tool_spec["inputSchema"]["json"],
            lookup_tool(Strict::Prefer).parameters
        );
    }

    #[tokio::test]
    async fn tool_choice_auto_and_none() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("Use the tool".to_string()),
                timestamp: TS,
            })],
            tools: Some(vec![Tool {
                name: "lookup".to_string(),
                description: "Look up a value".to_string(),
                parameters: json!({ "type": "object", "properties": {} }),
                constrained_sampling: None,
            }]),
        });
        let env = signing_env("us-east-1");

        // Auto: toolChoice {"auto":{}} present.
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                cache_retention: Some(CacheRetention::None),
                env: signing_env("us-east-1"),
                ..StreamOptions::default()
            },
            tool_choice: Some(ToolChoice::Auto),
            ..SimpleStreamOptions::default()
        };
        let (_, request) = capture_simple(&server, &claude_sonnet_4_5(), &ctx, &options).await;
        assert_eq!(
            request_body(&request)["toolConfig"]["toolChoice"],
            json!({ "auto": {} })
        );

        // None: the whole toolConfig is omitted.
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                cache_retention: Some(CacheRetention::None),
                env: signing_env("us-east-1"),
                ..StreamOptions::default()
            },
            tool_choice: Some(ToolChoice::None),
            ..SimpleStreamOptions::default()
        };
        let (_, request) = capture_simple(&server, &claude_sonnet_4_5(), &ctx, &options).await;
        assert!(request_body(&request).get("toolConfig").is_none());
        let _ = env;
    }

    #[tokio::test]
    async fn unsupported_strict_require_fails_the_stream() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("Use the tool".to_string()),
                timestamp: TS,
            })],
            // An unsupported schema shape ("require" on a boolean schema).
            tools: Some(vec![Tool {
                name: "lookup".to_string(),
                description: "Look up a value".to_string(),
                parameters: json!(true),
                constrained_sampling: Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                    strict: Strict::Require,
                })),
            }]),
        });
        let env = signing_env("us-east-1");
        let mut model = claude_sonnet_4_5();
        model.base_url = server.uri();
        let mut rx = BedrockConverseStream.stream(&cfg(), &model, &ctx, &stream_options(env));
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let message = error_of(&events);
        assert_eq!(
            message.error_message.as_deref(),
            Some(
                "Tool \"lookup\" requires JSON-schema constrained sampling, but root schema must have type object."
            ),
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    // =========================================================================
    // Custom headers (custom-headers oracle)
    // =========================================================================

    #[tokio::test]
    async fn custom_headers_injected_and_reserved_headers_skipped() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let mut headers = BTreeMap::new();
        headers.insert("authorization".to_string(), Some("evil".to_string()));
        headers.insert("x-amz-date".to_string(), Some("evil".to_string()));
        headers.insert("HOST".to_string(), Some("evil".to_string()));
        headers.insert("x-allowed".to_string(), Some("ok".to_string()));
        let options = StreamOptions {
            cache_retention: Some(CacheRetention::None),
            headers: Some(headers),
            env: Some(env_map(&[
                ("AWS_BEDROCK_SKIP_AUTH", "1"),
                ("AWS_REGION", "us-east-1"),
            ])),
            ..StreamOptions::default()
        };
        let (events, request) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &options,
        )
        .await;
        done_of(&events);
        let request_headers: Vec<(String, String)> = request
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap().to_string(),
                )
            })
            .collect();
        // Allowed header injected.
        assert!(
            request_headers
                .iter()
                .any(|(name, value)| name == "x-allowed" && value == "ok"),
            "{request_headers:?}"
        );
        // Reserved headers were never overridden by the evil values.
        let authorization = request_headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert!(
            authorization.starts_with("AWS4-HMAC-SHA256 "),
            "{authorization}"
        );
        assert_ne!(
            request_headers
                .iter()
                .find(|(name, _)| name == "x-amz-date")
                .map(|(_, value)| value.as_str()),
            Some("evil")
        );
        assert_ne!(
            request_headers
                .iter()
                .find(|(name, _)| name == "host")
                .map(|(_, value)| value.as_str()),
            Some("evil")
        );
        // No case-variant duplicates leaked through.
        assert!(!request_headers.iter().any(|(name, value)| {
            matches!(name.as_str(), "x-amz-date" | "authorization" | "host") && value == "evil"
        }));
    }

    #[tokio::test]
    async fn stream_simple_forwards_headers_end_to_end() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let mut headers = BTreeMap::new();
        headers.insert("x-custom".to_string(), Some("v".to_string()));
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                cache_retention: Some(CacheRetention::None),
                headers: Some(headers),
                env: Some(env_map(&[
                    ("AWS_BEDROCK_SKIP_AUTH", "1"),
                    ("AWS_REGION", "us-east-1"),
                ])),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (events, request) = capture_simple(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &options,
        )
        .await;
        done_of(&events);
        assert_eq!(header_value(&request, "x-custom"), "v");
    }

    #[tokio::test]
    async fn bearer_token_replaces_sigv4() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let options = StreamOptions {
            cache_retention: Some(CacheRetention::None),
            api_key: Some("bedrock-api-key".to_string()),
            env: signing_env("us-east-1"),
            ..StreamOptions::default()
        };
        let (events, request) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &options,
        )
        .await;
        done_of(&events);
        assert_eq!(
            header_value(&request, "authorization"),
            "Bearer bedrock-api-key"
        );
        // No SigV4 headers when the bearer scheme is selected.
        assert!(request.headers.get("x-amz-date").is_none());
        assert!(request.headers.get("x-amz-security-token").is_none());
    }

    #[tokio::test]
    async fn session_token_signed_into_request() {
        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let env = env_map(&[
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "SECRET"),
            ("AWS_SESSION_TOKEN", "TOKEN"),
            ("AWS_REGION", "us-east-1"),
        ]);
        let (events, request) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &stream_options(Some(env)),
        )
        .await;
        done_of(&events);
        assert_eq!(header_value(&request, "x-amz-security-token"), "TOKEN");
        assert!(header_value(&request, "authorization").starts_with("AWS4-HMAC-SHA256 "));
    }

    // =========================================================================
    // Failure diagnostics (error-metadata oracle)
    // =========================================================================

    async fn serve_and_capture(
        status: u16,
        body: Vec<u8>,
        headers: Vec<(&str, &str)>,
    ) -> AssistantMessage {
        let server = wiremock::MockServer::start().await;
        let mut template = wiremock::ResponseTemplate::new(status).set_body_bytes(body);
        for (name, value) in headers {
            template = template.insert_header(name, value);
        }
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(template)
            .mount(&server)
            .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        error_of(&events)
    }

    #[tokio::test]
    async fn http_error_records_status_code_and_request_id() {
        let message = serve_and_capture(
            400,
            json!({ "__type": "ValidationException", "message": "The provided model identifier is invalid." })
                .to_string()
                .into_bytes(),
            vec![("x-amzn-requestid", "11111111-2222-3333-4444-555555555555")],
        )
        .await;
        assert_eq!(message.stop_reason, StopReason::Error);
        // errorMessage untouched: the retry classifier keeps matching.
        assert_eq!(
            message.error_message.as_deref(),
            Some("Validation error: The provided model identifier is invalid.")
        );
        let diagnostic = diagnostic_of(&message);
        assert_eq!(
            diagnostic,
            &json!({
                "status": 400,
                "errorCode": "ValidationException",
                "requestId": "11111111-2222-3333-4444-555555555555"
            })
        );
    }

    #[tokio::test]
    async fn mid_stream_modeled_exception_reports_only_request_id() {
        // The SDK throws a bare object literal here, so the code is genuinely
        // unavailable: no prefix, no errorCode.
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                exception_frame(
                    "throttlingException",
                    &json!({ "message": "Too many requests, please wait." }),
                ),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        let message = error_of(&events);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(diagnostic_of(&message), &json!({ "requestId": "req-123" }));
    }

    #[tokio::test]
    async fn unmodeled_error_frame_captures_the_error_code() {
        // A real `Error` named after the frame's `:error-code`.
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                error_frame(
                    "ModelStreamErrorException",
                    "Model stream terminated unexpectedly.",
                ),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        let message = error_of(&events);
        assert_eq!(
            diagnostic_of(&message),
            &json!({
                "errorCode": "ModelStreamErrorException",
                "requestId": "req-123"
            })
        );
        assert_eq!(
            message.error_message.as_deref(),
            Some("Model stream terminated unexpectedly.")
        );
    }

    #[tokio::test]
    async fn transport_names_are_not_provider_error_codes() {
        let server = wiremock::MockServer::start().await;
        serve_frames(
            &server,
            &[
                event_frame("messageStart", &json!({ "role": "assistant" })),
                error_frame("TimeoutError", "Connection timed out after 1000 ms"),
            ],
        )
        .await;
        let env = signing_env("us-east-1");
        let (events, _) = capture_stream(
            &server,
            &claude_sonnet_4_5(),
            &user_context("hello"),
            &stream_options(env),
        )
        .await;
        let message = error_of(&events);
        assert_eq!(diagnostic_of(&message), &json!({ "requestId": "req-123" }));
    }

    #[tokio::test]
    async fn over_long_header_values_are_dropped_not_truncated() {
        let long_name = format!("{}Exception", "E".repeat(5000));
        let long_request_id = "R".repeat(5000);
        let message = serve_and_capture(
            400,
            json!({ "__type": long_name, "message": "invalid" })
                .to_string()
                .into_bytes(),
            vec![("x-amzn-requestid", long_request_id.as_str())],
        )
        .await;
        assert_eq!(diagnostic_of(&message), &json!({ "status": 400 }));
    }

    #[tokio::test]
    async fn unknown_gateway_error_surfaces_status_and_body() {
        // No AWS error shape: the SDK's Unknown placeholder, with the raw
        // body surfaced through the formatBedrockError body path.
        let message = serve_and_capture(
            403,
            b"Forbidden".to_vec(),
            vec![("x-amzn-requestid", "req-403")],
        )
        .await;
        assert_eq!(
            message.error_message.as_deref(),
            Some("Unknown: 403: Forbidden")
        );
        assert_eq!(
            diagnostic_of(&message),
            &json!({ "status": 403, "requestId": "req-403" })
        );
    }

    #[tokio::test]
    async fn data_retention_errors_get_the_docs_hint() {
        let message = serve_and_capture(
            400,
            json!({
                "__type": "ValidationException",
                "message": "data retention mode 'default' is not available for this model"
            })
            .to_string()
            .into_bytes(),
            vec![],
        )
        .await;
        assert_eq!(
            message.error_message.as_deref(),
            Some(format!(
                "Validation error: data retention mode 'default' is not available for this model See {BEDROCK_DATA_RETENTION_DOCS_URL} for supported data retention modes."
            )
            .as_str())
        );
    }

    #[tokio::test]
    async fn missing_credentials_fail_before_the_request() {
        // Clear ambient AWS credential env so the failure is deterministic.
        struct EnvGuard(Vec<(String, Option<String>)>);
        impl EnvGuard {
            fn sanitize() -> Self {
                let mut saved = Vec::new();
                for name in [
                    "AWS_ACCESS_KEY_ID",
                    "AWS_SECRET_ACCESS_KEY",
                    "AWS_SESSION_TOKEN",
                    "AWS_PROFILE",
                    "AWS_BEARER_TOKEN_BEDROCK",
                    "AWS_BEDROCK_SKIP_AUTH",
                ] {
                    saved.push((name.to_string(), std::env::var(name).ok()));
                    std::env::remove_var(name);
                }
                EnvGuard(saved)
            }
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for (name, value) in &self.0 {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
        let _guard = EnvGuard::sanitize();

        let server = wiremock::MockServer::start().await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let mut model = claude_sonnet_4_5();
        model.base_url = server.uri();
        let options = StreamOptions {
            cache_retention: Some(CacheRetention::None),
            ..StreamOptions::default()
        };
        let mut rx = BedrockConverseStream.stream(&cfg(), &model, &user_context("hi"), &options);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let message = error_of(&events);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Could not load credentials from any providers")
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    // =========================================================================
    // Retry seam
    // =========================================================================

    #[tokio::test]
    async fn retryable_500_is_retried_then_succeeds() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(500).set_body_bytes(
                json!({ "__type": "InternalServerException", "message": "boom" }).to_string(),
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        serve_frames(&server, &text_stream_frames("ok", "end_turn")).await;
        let options = StreamOptions {
            cache_retention: Some(CacheRetention::None),
            max_retries: Some(1),
            env: signing_env("us-east-1"),
            ..StreamOptions::default()
        };
        let (events, _) =
            capture_stream(&server, &claude_sonnet_4_5(), &user_context("hi"), &options).await;
        let (_, message) = done_of(&events);
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn non_retryable_400_is_not_retried() {
        serve_and_capture(
            400,
            json!({ "__type": "ValidationException", "message": "nope" })
                .to_string()
                .into_bytes(),
            vec![],
        )
        .await;
        // The capture above used the default (no retry); rerun with one retry
        // allowed to prove the policy keeps 400s single-shot.
        let server = wiremock::MockServer::start().await;
        serve_bytes(
            &server,
            400,
            json!({ "__type": "ValidationException", "message": "nope" })
                .to_string()
                .into_bytes(),
        )
        .await;
        let options = StreamOptions {
            cache_retention: Some(CacheRetention::None),
            max_retries: Some(1),
            env: signing_env("us-east-1"),
            ..StreamOptions::default()
        };
        let (events, _) =
            capture_stream(&server, &claude_sonnet_4_5(), &user_context("hi"), &options).await;
        error_of(&events);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    // =========================================================================
    // Pure formatting/diagnostic helpers
    // =========================================================================

    #[test]
    fn format_bedrock_error_prefixes_service_exceptions() {
        let failure = BedrockFailure {
            name: Some("ThrottlingException".to_string()),
            service_exception: true,
            message: "Rate limited".to_string(),
            status: Some(429),
            body: None,
            metadata_request_id: None,
        };
        assert_eq!(
            format_bedrock_error(&failure),
            "Throttling error: Rate limited"
        );

        // Unmodeled service exception names pass through verbatim.
        let failure = BedrockFailure {
            name: Some("CustomException".to_string()),
            service_exception: true,
            message: "nope".to_string(),
            status: Some(400),
            body: None,
            metadata_request_id: None,
        };
        assert_eq!(format_bedrock_error(&failure), "CustomException: nope");

        // Non-service failures keep the bare message.
        let failure = BedrockFailure::plain("socket hang up");
        assert_eq!(format_bedrock_error(&failure), "socket hang up");
    }

    #[test]
    fn normalize_diagnostic_value_bounds() {
        assert_eq!(
            normalize_diagnostic_value(Some("  id  ")),
            Some("id".to_string())
        );
        assert_eq!(normalize_diagnostic_value(Some("   ")), None);
        assert_eq!(normalize_diagnostic_value(Some(&"x".repeat(201))), None);
        assert_eq!(
            normalize_diagnostic_value(Some(&"x".repeat(200))),
            Some("x".repeat(200))
        );
        assert_eq!(normalize_diagnostic_value(None), None);
    }

    #[test]
    fn error_code_requires_exception_suffix() {
        let failure = BedrockFailure {
            name: Some("ValidationException".to_string()),
            service_exception: true,
            message: String::new(),
            status: None,
            body: None,
            metadata_request_id: None,
        };
        assert_eq!(
            extract_bedrock_error_code(&failure),
            Some("ValidationException".to_string())
        );
        let failure = BedrockFailure {
            name: Some("TimeoutError".to_string()),
            ..failure
        };
        assert_eq!(extract_bedrock_error_code(&failure), None);
        assert_eq!(
            extract_bedrock_error_code(&BedrockFailure::plain("x")),
            None
        );
    }

    #[test]
    fn stop_reason_mapping_table() {
        assert_eq!(map_stop_reason(Some("end_turn")), (StopReason::Stop, None));
        assert_eq!(
            map_stop_reason(Some("stop_sequence")),
            (StopReason::Stop, None)
        );
        assert_eq!(
            map_stop_reason(Some("max_tokens")),
            (StopReason::Length, None)
        );
        assert_eq!(
            map_stop_reason(Some("model_context_window_exceeded")),
            (StopReason::Length, None)
        );
        assert_eq!(
            map_stop_reason(Some("tool_use")),
            (StopReason::ToolUse, None)
        );
        assert_eq!(
            map_stop_reason(Some("guardrail_intervened")),
            (
                StopReason::Error,
                Some("Provider stopped with: guardrail_intervened".to_string())
            )
        );
        // Upstream falsy semantics: empty and missing map to a bare error.
        assert_eq!(map_stop_reason(Some("")), (StopReason::Error, None));
        assert_eq!(map_stop_reason(None), (StopReason::Error, None));
    }

    #[test]
    fn model_predicates_match_candidates() {
        // Name-only Claude detection (application inference profiles).
        let mut profile = model(
            "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile",
            "Claude Sonnet 4.6",
        );
        profile.compat = None;
        assert!(is_anthropic_claude_model(&profile));
        assert!(supports_adaptive_thinking(&profile));
        assert!(supports_prompt_caching(&profile, None));
        assert!(!supports_native_xhigh_effort(&profile));

        // Separator normalization: spaces, dots, underscores, colons.
        let mut spaced = model("Claude Opus 4.8 (Global)", "anything");
        spaced.compat = None;
        assert!(supports_adaptive_thinking(&spaced));

        // Non-Claude models.
        assert!(!is_anthropic_claude_model(&gpt_model()));
        assert!(!supports_adaptive_thinking(&gpt_model()));
        assert!(!supports_prompt_caching(&gpt_model(), None));
    }

    #[test]
    fn tool_call_id_normalization() {
        assert_eq!(normalize_tool_call_id("tool_abc-123"), "tool_abc-123");
        assert_eq!(normalize_tool_call_id("call.1/2:3"), "call_1_2_3");
        let long = "a".repeat(80);
        assert_eq!(normalize_tool_call_id(&long), "a".repeat(64));
    }

    #[test]
    fn document_sanitization_strips_empty_keys_recursively() {
        let document = json!({
            "path": "/tmp/x",
            "": "dropped",
            "nested": { "keep": 1, "": { "dropped": true } },
            "list": [{ "ok": true, "": null }, "scalar"]
        });
        assert_eq!(
            sanitize_bedrock_document(&document),
            json!({
                "path": "/tmp/x",
                "nested": { "keep": 1 },
                "list": [{ "ok": true }, "scalar"]
            })
        );
        // Scalars pass through.
        assert_eq!(sanitize_bedrock_document(&json!(42)), json!(42));
    }

    #[test]
    fn image_format_resolution() {
        assert_eq!(
            create_image_block("image/jpg", "aGk=").unwrap(),
            json!({ "image": { "source": { "bytes": "aGk=" }, "format": "jpeg" } })
        );
        assert_eq!(
            create_image_block("image/webp", "aGk=").unwrap()["image"]["format"],
            json!("webp")
        );
        assert_eq!(
            create_image_block("image/tiff", "aGk=")
                .unwrap_err()
                .message,
            "Unknown image type: image/tiff"
        );
    }

    #[test]
    fn cache_retention_env_fallback() {
        assert_eq!(
            resolve_cache_retention(Some(CacheRetention::Long), None),
            CacheRetention::Long
        );
        assert_eq!(resolve_cache_retention(None, None), CacheRetention::Short);
        let env = env_map(&[("PI_CACHE_RETENTION", "long")]);
        assert_eq!(
            resolve_cache_retention(None, Some(&env)),
            CacheRetention::Long
        );
        let env = env_map(&[("PI_CACHE_RETENTION", "short")]);
        assert_eq!(
            resolve_cache_retention(None, Some(&env)),
            CacheRetention::Short
        );
    }

    #[test]
    fn thinking_effort_map_and_fallbacks() {
        let base = claude_sonnet_4_5();
        assert_eq!(
            map_thinking_level_to_effort(&base, Some(ThinkingLevel::Low)),
            "low"
        );
        assert_eq!(
            map_thinking_level_to_effort(&base, Some(ThinkingLevel::Medium)),
            "medium"
        );
        assert_eq!(
            map_thinking_level_to_effort(&base, Some(ThinkingLevel::High)),
            "high"
        );
        assert_eq!(
            map_thinking_level_to_effort(&base, Some(ThinkingLevel::Max)),
            "high"
        );
        assert_eq!(map_thinking_level_to_effort(&base, None), "high");
        // xhigh is native only on the newest line.
        let opus = model("global.anthropic.claude-opus-4-8-v1", "Opus 4.8");
        assert_eq!(
            map_thinking_level_to_effort(&opus, Some(ThinkingLevel::Xhigh)),
            "xhigh"
        );
        assert_eq!(
            map_thinking_level_to_effort(&base, Some(ThinkingLevel::Xhigh)),
            "high"
        );
    }

    #[test]
    fn extended_encoding_matches_js_component_encoding() {
        assert_eq!(extended_encode_uri_component("aB9-_.~"), "aB9-_.~");
        assert_eq!(extended_encode_uri_component(" "), "%20");
        assert_eq!(extended_encode_uri_component(":"), "%3A");
        assert_eq!(extended_encode_uri_component("ä"), "%C3%A4");
    }

    #[test]
    fn usage_total_falls_back_to_input_plus_output() {
        // Covered through handle_metadata: totalTokens 0 falls back to the sum.
        let mut state = StreamState::new(&claude_sonnet_4_5());
        handle_metadata(
            &mut state,
            &json!({ "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 0 } }),
            &claude_sonnet_4_5(),
        );
        assert_eq!(state.output.usage.total_tokens, 15);
        assert_eq!(state.output.usage.cache_write_1h, None);
        // Present-but-empty cacheDetails still emits the 1h bucket at zero.
        handle_metadata(
            &mut state,
            &json!({ "usage": { "cacheDetails": [] } }),
            &claude_sonnet_4_5(),
        );
        assert_eq!(state.output.usage.cache_write_1h, Some(0));
    }

    // ---- abort surface ----

    fn aborted_options() -> StreamOptions {
        let token = CancellationToken::new();
        token.cancel();
        StreamOptions {
            signal: Some(token),
            env: signing_env("us-east-1"),
            ..StreamOptions::default()
        }
    }

    /// A pre-cancelled signal fails the request setup (the retry seam's
    /// `"Request aborted"` abort error) before `Start`, and the catch block
    /// settles `stopReason: "aborted"`.
    #[tokio::test]
    async fn pre_aborted_request_settles_aborted_before_start() {
        let server = wiremock::MockServer::start().await;
        let model = model("anthropic.claude-sonnet-4-5", "Claude Sonnet 4.5");
        let api = BedrockConverseStream;
        let mut rx = api.stream(&cfg(), &model, &user_context("hi"), &aborted_options());
        let first = rx.recv().await.expect("terminal event");
        match first {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some(REQUEST_ABORTED));
            }
            other => panic!("expected terminal error event, got {other:?}"),
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// A cancellation after `start` breaks the frame-read loop and settles
    /// the stream aborted with `"Request was aborted"` (upstream line 330;
    /// the SDK's `abortSignal` breaks body reads). The raw TCP server writes
    /// the response head plus one `messageStart` frame and then stalls, so
    /// only the abort path can end the stream.
    #[tokio::test]
    async fn mid_stream_cancellation_settles_the_stream_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 16384];
            let _ = socket.read(&mut buf).await;
            let head = "HTTP/1.1 200 OK\r\ncontent-type: application/vnd.amazon.eventstream\r\nconnection: close\r\n\r\n";
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket
                .write_all(&event_frame(
                    "messageStart",
                    &json!({ "role": "assistant" }),
                ))
                .await;
            // Hold the socket open (no further bytes) until the runtime
            // drops the task.
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
        let mut model = model("anthropic.claude-sonnet-4-5", "Claude Sonnet 4.5");
        model.base_url = format!("http://{addr}");
        let token = CancellationToken::new();
        let options = StreamOptions {
            signal: Some(token.clone()),
            env: signing_env("us-east-1"),
            ..StreamOptions::default()
        };
        let api = BedrockConverseStream;
        let mut rx = api.stream(&cfg(), &model, &user_context("hi"), &options);
        let first = rx.recv().await.expect("start event");
        assert!(matches!(first, AssistantMessageEvent::Start { .. }));
        token.cancel();
        match rx.recv().await.expect("terminal event") {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some(REQUEST_WAS_ABORTED));
            }
            other => panic!("expected terminal error event, got {other:?}"),
        }
    }
}
