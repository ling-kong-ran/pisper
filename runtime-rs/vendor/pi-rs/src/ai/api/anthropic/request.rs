//! Anthropic-messages request assembly — full port of the request-body and
//! header assembly from upstream `packages/ai/src/api/anthropic-messages.ts`:
//!
//! - `buildParams` (lines 1035-1205): model, messages, max_tokens, system
//!   blocks, temperature gating, tools (native tool changes with deferred
//!   loading), thinking/output-config variants, metadata, tool choice, and
//!   fallbacks.
//! - `convertMessages` (lines 1226-1428) plus `convertContentBlocks`
//!   (lines 128-175), `convertToolResult` (lines 1212-1219), and the trailing
//!   cache-control marker on the last message (lines 1399-1425).
//! - `convertTools` (lines 1457-1492) with the constrained-sampling helpers of
//!   `api/constrained-sampling.ts` (shared with the openai-completions port).
//! - `getBetaFeatures` (lines 991-1033), the beta-header constants
//!   (lines 181-186), and `DEFERRED_TOOL_PLACEHOLDER` (lines 195-200).
//! - `getCacheControl`/`resolveCacheRetention` (lines 60-84) and
//!   `getAnthropicCompat` (lines 206-220) with its upstream defaults.
//! - `createClient` header assembly (lines 910-989) without the HTTP client:
//!   the pi User-Agent (Claude Code identity for OAuth), accept/UA defaults,
//!   session-affinity headers, copilot dynamic headers, model headers, and the
//!   caller's headers merged last, plus the SDK-injected `x-api-key` /
//!   `Authorization: Bearer` auth pair and `anthropic-version`.
//! - Claude Code tool-name normalization (lines 86-123) applied to tool names
//!   and `tool_use`/`tool_removal`/`tool_addition` references for OAuth.
//! - `streamSimple` base-option shaping (`api/simple-options.ts`): context
//!   clamping of `maxTokens`, `adjustMaxTokensForThinking`, and
//!   `mapThinkingLevelToEffort` (lines 838-856) behind [`options_from_simple`].
//!
//! The entry point is pure: it returns the JSON body and the ordered header
//! pairs an HTTP layer would send, it never performs I/O. Errors mirror
//! upstream throws: missing credentials (`assertRequestAuth`,
//! lines 307-317) and strict-sampling requests that cannot be honored.
//!
//! Deviations from upstream, all structural:
//! - `sanitizeSurrogates` (`utils/sanitize-unicode.ts`) is a no-op: Rust
//!   `String` is UTF-8 and cannot hold unpaired surrogates, so the JS cleanup
//!   pass has no input it could act on.
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order; servers do not care and every body assertion here is
//!   order-insensitive. The one visible consequence: the strict-tool
//!   `required` array follows the sorted property order rather than the
//!   schema's insertion order (pinned by a test).
//! - `normalizeToolCallId` (line 1207-1210) replaces per UTF-16 code unit and
//!   slices to 64, matching JS byte-for-byte.
//! - The credential resolution accepts the provider-level `cfg.api_key` as a
//!   fallback for `options.apiKey` (upstream reads only `options.apiKey`);
//!   this is the port's wiring for the [`crate::ai::ProviderConfig`] the
//!   [`crate::ai::api::ApiImpl`] signature carries.
//! - The shared transcript/option helpers (`transformMessages`,
//!   `clampMaxTokensToContext`, `thinkingBudgetForLevel`,
//!   `renderSystemMessageUpdate`, strict-sampling) are reused from the
//!   openai-completions port instead of being duplicated; they are the same
//!   upstream code paths.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

use crate::ai::api::openai_completions::request::{
    clamp_max_tokens_to_context, make_strict_json_schema, render_system_message_update,
    resolve_json_schema_strict_sampling, thinking_budget_for_level, transform_messages,
    MIN_ANSWER_TOKENS,
};
use crate::ai::api::pi_user_agent;
use crate::ai::transcript::{
    get_current_tools, get_initial_system_message, get_system_message_text, resolve_transcript,
    TranscriptContext,
};
use crate::ai::types::message::{
    AssistantBlock, Message, StringOrBlocks, TextOrImageBlock, ToolResultMessage,
};
use crate::ai::types::options::{ProviderEnv, ProviderHeaders, SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{CacheRetention, ThinkingBudgets, ThinkingLevel, ToolChoice};
use crate::ai::types::tool::Tool;
use crate::ai::types::Model;
use crate::ai::ProviderConfig;

/// The v1.0.0 `convertToolDefinitions` closure handed to `convertMessages`.
type ConvertToolDefinitionsFn<'a> = dyn Fn(&[Tool]) -> Result<Vec<Value>, String> + 'a;
type ConvertToolDefinitions<'a> = Box<ConvertToolDefinitionsFn<'a>>;

/// The pure output of [`build_request`]: the JSON request body and the ordered
/// header pairs from `buildParams` + `createClient`. The body retains SDK
/// params (including `betas`); the stream transport materializes SDK-only
/// header fields after onPayload, before putting the body on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestAssembly {
    pub body: Value,
    pub headers: Vec<(String, String)>,
}

/// Upstream `AnthropicEffort` (anthropic-messages.ts:177).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl AnthropicEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            AnthropicEffort::Low => "low",
            AnthropicEffort::Medium => "medium",
            AnthropicEffort::High => "high",
            AnthropicEffort::Xhigh => "xhigh",
            AnthropicEffort::Max => "max",
        }
    }

    /// Upstream `isAnthropicEffort` (lines 1430-1432) as a parse.
    pub fn from_str_opt(value: &str) -> Option<Self> {
        match value {
            "low" => Some(AnthropicEffort::Low),
            "medium" => Some(AnthropicEffort::Medium),
            "high" => Some(AnthropicEffort::High),
            "xhigh" => Some(AnthropicEffort::Xhigh),
            "max" => Some(AnthropicEffort::Max),
            _ => None,
        }
    }
}

/// Upstream `AnthropicThinkingDisplay` (anthropic-messages.ts:179).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicThinkingDisplay {
    Summarized,
    Omitted,
}

impl AnthropicThinkingDisplay {
    pub fn as_str(self) -> &'static str {
        match self {
            AnthropicThinkingDisplay::Summarized => "summarized",
            AnthropicThinkingDisplay::Omitted => "omitted",
        }
    }
}

/// Upstream `AnthropicOptions.toolChoice` (anthropic-messages.ts:271-275):
/// the string choices map to Anthropic's built-ins; the object form forces a
/// specific tool.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum AnthropicToolChoice {
    Auto,
    Any,
    None,
    Tool { name: String },
}

impl AnthropicToolChoice {
    /// The request-body shape: strings become `{ type: value }`.
    fn to_wire(&self) -> Value {
        match self {
            AnthropicToolChoice::Auto => json!({"type": "auto"}),
            AnthropicToolChoice::Any => json!({"type": "any"}),
            AnthropicToolChoice::None => json!({"type": "none"}),
            AnthropicToolChoice::Tool { name } => json!({"type": "tool", "name": name}),
        }
    }
}

impl From<ToolChoice> for AnthropicToolChoice {
    fn from(choice: ToolChoice) -> Self {
        match choice {
            ToolChoice::Auto => AnthropicToolChoice::Auto,
            ToolChoice::None => AnthropicToolChoice::None,
        }
    }
}

/// Upstream `AnthropicOptions` (anthropic-messages.ts:222-282) minus the
/// client/signal/fetch/callback fields that land with the M2b stream port:
/// the base [`StreamOptions`] flattened with the API-specific extensions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnthropicOptions {
    /// Base options inherited from upstream `StreamOptions`.
    pub stream: StreamOptions,
    /// Enable extended thinking (anthropic-messages.ts:229-238). `None`
    /// (upstream `undefined`) omits the thinking param entirely unless
    /// `streamSimple` maps a reasoning level onto it.
    pub thinking_enabled: Option<bool>,
    /// Token budget for extended thinking on older models
    /// (anthropic-messages.ts:239-243).
    pub thinking_budget_tokens: Option<u64>,
    /// Effort level for adaptive thinking models (anthropic-messages.ts:244-256).
    pub effort: Option<AnthropicEffort>,
    /// How thinking content is returned (anthropic-messages.ts:257-269);
    /// defaults to `"summarized"` when thinking is enabled.
    pub thinking_display: Option<AnthropicThinkingDisplay>,
    /// Request the interleaved-thinking beta for non-adaptive models
    /// (anthropic-messages.ts:270-276); defaults to true.
    pub interleaved_thinking: Option<bool>,
    /// Anthropic tool choice behavior (anthropic-messages.ts:277-282).
    pub tool_choice: Option<AnthropicToolChoice>,
}

impl AnthropicOptions {
    /// Base options only, for direct `stream`-path callers that pass no
    /// API-specific extensions (upstream: `options` fields all `undefined`).
    pub fn from_stream(stream: StreamOptions) -> Self {
        AnthropicOptions {
            stream,
            ..AnthropicOptions::default()
        }
    }
}

// ---- beta constants (anthropic-messages.ts:181-186) ----

const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MID_CONVERSATION_OUTPUT_CONFIG_BETA: &str = "mid-conversation-output-config-2026-07-01";
const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";
/// v1.0.0: the mid-conversation tool-changes beta became the inline-tools
/// beta (`tool_addition` blocks carry inline tool definitions).
const INLINE_TOOLS_BETA: &str = "inline-tools-2026-09-15";

/// Claude Code identity for OAuth requests (anthropic-messages.ts:87, 1082).
const CLAUDE_CODE_VERSION: &str = "2.1.280";
const CLAUDE_CODE_IDENTITY_PROMPT: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";
/// SDK-injected `anthropic-version` header value.
const ANTHROPIC_VERSION_HEADER: &str = "2023-06-01";
/// Image-only tool-result placeholder (anthropic-messages.ts:171).
const IMAGE_ONLY_TOOL_RESULT_PLACEHOLDER: &str = "(see attached image)";

// ---- Claude Code stealth naming (anthropic-messages.ts:86-123) ----

/// Claude Code 2.x canonical tool names (anthropic-messages.ts:92-110).
const CLAUDE_CODE_TOOLS: [&str; 17] = [
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

/// Upstream `toClaudeCodeName` (line 115): CC canonical casing for
/// case-insensitive matches, passthrough otherwise.
fn to_claude_code_name(name: &str) -> String {
    let lowered = name.to_lowercase();
    CLAUDE_CODE_TOOLS
        .iter()
        .find(|tool| tool.to_lowercase() == lowered)
        .map(|tool| (*tool).to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Upstream `normalizeToolCallId` (lines 1207-1210): replace every
/// non-`[a-zA-Z0-9_-]` UTF-16 code unit with `_` (astral characters are two
/// units, like the JS regex), then clamp to 64.
fn normalize_tool_call_id(id: &str) -> String {
    let mut replaced = String::with_capacity(id.len());
    for unit in id.encode_utf16() {
        let ch = match unit {
            0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A | 0x5F | 0x2D => {
                char::from_u32(u32::from(unit)).unwrap_or('_')
            }
            _ => '_',
        };
        replaced.push(ch);
    }
    replaced.chars().take(64).collect()
}

// ---- compat resolution (upstream getAnthropicCompat, lines 206-220) ----

/// The resolved compat flags with their upstream defaults applied. Read from
/// the raw compat object so one invalid key cannot silently drop every
/// setting (upstream reads fields off a plain JS object). Shared with the
/// stream port, which reads the transcript-resolution and effort-stamping
/// flags and the fallback cost lookup inputs.
pub(crate) struct AnthropicCompat {
    supports_eager_tool_input_streaming: bool,
    supports_long_cache_retention: bool,
    send_session_affinity_headers: bool,
    session_affinity_is_openrouter: bool,
    supports_cache_control_on_tools: bool,
    supports_temperature: bool,
    allow_empty_signature: bool,
    supports_strict_tools: bool,
    pub(crate) supports_mid_convo_system_messages: bool,
    supports_mid_convo_tool_changes: bool,
    force_adaptive_thinking: Option<bool>,
    pub(crate) supports_mid_convo_effort: bool,
    /// `model` ids of `allowedFallbackModels` (upstream maps to `{ model }`).
    allowed_fallback_models: Vec<String>,
}

pub(crate) fn get_anthropic_compat(model: &Model) -> AnthropicCompat {
    let compat = model.compat.as_ref();
    let flag =
        |key: &str| -> Option<bool> { compat.and_then(|c| c.get(key)).and_then(Value::as_bool) };
    // OpenRouter defaults (line 207).
    let is_openrouter = model.provider == "openrouter" || model.base_url.contains("openrouter.ai");
    let explicit_format = compat
        .and_then(|c| c.get("sessionAffinityFormat"))
        .and_then(Value::as_str);
    AnthropicCompat {
        supports_eager_tool_input_streaming: flag("supportsEagerToolInputStreaming")
            .unwrap_or(true),
        supports_long_cache_retention: flag("supportsLongCacheRetention").unwrap_or(true),
        send_session_affinity_headers: flag("sendSessionAffinityHeaders").unwrap_or(is_openrouter),
        session_affinity_is_openrouter: match explicit_format {
            Some("openrouter") => true,
            Some(_) => false,
            None => is_openrouter,
        },
        supports_cache_control_on_tools: flag("supportsCacheControlOnTools").unwrap_or(true),
        supports_temperature: flag("supportsTemperature").unwrap_or(true),
        allow_empty_signature: flag("allowEmptySignature").unwrap_or(false),
        supports_strict_tools: flag("supportsStrictTools").unwrap_or(false),
        supports_mid_convo_system_messages: flag("supportsMidConvoSystemMessages").unwrap_or(false),
        supports_mid_convo_tool_changes: flag("supportsMidConvoToolChanges").unwrap_or(false),
        force_adaptive_thinking: flag("forceAdaptiveThinking"),
        supports_mid_convo_effort: flag("supportsMidConvoEffort").unwrap_or(false),
        allowed_fallback_models: compat
            .and_then(|c| c.get("allowedFallbackModels"))
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(|fallback| fallback.get("model"))
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

// ---- auth (upstream hasHeader + assertRequestAuth, lines 298-317) ----

/// Upstream `hasHeader`: case-insensitive presence with a non-empty value.
fn has_header(headers: Option<&ProviderHeaders>, name: &str) -> bool {
    headers.into_iter().flatten().any(|(key, value)| {
        key.eq_ignore_ascii_case(name)
            && value
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    })
}

/// Upstream `assertRequestAuth` composed with the credential resolution: the
/// options key first (upstream `options?.apiKey`), then the provider
/// credential `cfg.api_key` (the port's wiring), then header-owned auth
/// (gateway `Authorization`/`x-api-key`/`cf-aig-authorization`). `None` means
/// header-owned auth: no auth header is injected.
pub(crate) fn resolve_api_key(
    model: &Model,
    cfg: &ProviderConfig,
    options: &AnthropicOptions,
) -> Result<Option<String>, String> {
    if let Some(key) = options
        .stream
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
    {
        return Ok(Some(key.to_string()));
    }
    if !cfg.api_key.is_empty() {
        return Ok(Some(cfg.api_key.clone()));
    }
    if has_header(options.stream.headers.as_ref(), "authorization")
        || has_header(options.stream.headers.as_ref(), "x-api-key")
        || has_header(options.stream.headers.as_ref(), "cf-aig-authorization")
    {
        return Ok(None);
    }
    Err(format!("No API key for provider: {}", model.provider))
}

/// Upstream `isOAuthToken` (line 906-908).
pub(crate) fn is_oauth_token(api_key: &str) -> bool {
    api_key.contains("sk-ant-oat")
}

// ---- cache control (upstream lines 60-84) ----

/// Upstream `resolveCacheRetention`: explicit option, then the scoped
/// `PI_CACHE_RETENTION` env value, then the process environment, then short.
fn resolve_cache_retention(
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

/// Upstream `getCacheControl`: the ephemeral marker with the 1h ttl when long
/// retention is requested and supported.
fn get_cache_control(compat: &AnthropicCompat, retention: CacheRetention) -> Option<Value> {
    if retention == CacheRetention::None {
        return None;
    }
    let ttl =
        (retention == CacheRetention::Long && compat.supports_long_cache_retention).then_some("1h");
    Some(match ttl {
        Some(ttl) => json!({"type": "ephemeral", "ttl": ttl}),
        None => json!({"type": "ephemeral"}),
    })
}

/// A system text block with the optional cache marker (lines 1079-1101).
fn system_text_block(text: &str, cache_control: Option<&Value>) -> Value {
    let mut block = json!({"type": "text", "text": text});
    if let Some(marker) = cache_control {
        block["cache_control"] = marker.clone();
    }
    block
}

/// Upstream `DEFERRED_TOOL_PLACEHOLDER` (lines 195-200).
fn deferred_tool_placeholder() -> Value {
    json!({
        "name": "__pi_deferred_placeholder__",
        "description": "Reserved placeholder. Never available. Never call this.",
        "input_schema": {"type": "object", "properties": {}, "required": []},
        "defer_loading": true
    })
}

// ---- beta features (upstream getBetaFeatures, lines 991-1033) ----

#[allow(clippy::too_many_arguments)]
fn get_beta_features(
    model: &Model,
    context: &TranscriptContext,
    is_oauth: bool,
    native_tool_changes: bool,
    options: &AnthropicOptions,
    compat: &AnthropicCompat,
) -> Vec<String> {
    // An explicit `anthropic-beta` header (model headers, then option headers)
    // replaces the computed feature list; an explicit null suppresses it
    // (lines 998-1014).
    let mut configured: Option<Option<String>> = None;
    for (name, value) in model.headers.iter().flatten() {
        if name.eq_ignore_ascii_case("anthropic-beta") {
            configured = Some(value.clone());
        }
    }
    for (name, value) in options.stream.headers.iter().flatten() {
        if name.eq_ignore_ascii_case("anthropic-beta") {
            configured = Some(value.clone());
        }
    }
    match configured {
        Some(None) => return Vec::new(),
        Some(Some(features)) => {
            let mut parsed: Vec<String> = Vec::new();
            for feature in features.split(',') {
                let feature = feature.trim();
                if !feature.is_empty() && !parsed.iter().any(|existing| existing == feature) {
                    parsed.push(feature.to_string());
                }
            }
            return parsed;
        }
        None => {}
    }

    let mut features: Vec<String> = Vec::new();
    if is_oauth {
        features.push("claude-code-20250219".to_string());
        features.push("oauth-2025-04-20".to_string());
    }
    // Legacy fine-grained tool streaming when eager input streaming is off
    // (`shouldUseFineGrainedToolStreamingBeta`, lines 1450-1455).
    if !get_current_tools(context.messages()).is_empty()
        && !compat.supports_eager_tool_input_streaming
    {
        features.push(FINE_GRAINED_TOOL_STREAMING_BETA.to_string());
    }
    if model.reasoning
        && options.thinking_enabled == Some(true)
        && options.interleaved_thinking.unwrap_or(true)
        && compat.force_adaptive_thinking != Some(true)
    {
        features.push(INTERLEAVED_THINKING_BETA.to_string());
    }
    if !compat.allowed_fallback_models.is_empty() {
        features.push(SERVER_SIDE_FALLBACK_BETA.to_string());
    }
    if compat.supports_mid_convo_effort {
        features.push(MID_CONVERSATION_OUTPUT_CONFIG_BETA.to_string());
        features.push(THINKING_BINDING_CONTROLS_BETA.to_string());
    }
    if native_tool_changes {
        features.push(INLINE_TOOLS_BETA.to_string());
    }
    // `[...new Set(features)]`.
    let mut unique: Vec<String> = Vec::new();
    for feature in features {
        if !unique.contains(&feature) {
            unique.push(feature);
        }
    }
    unique
}

/// Assemble the Anthropic Messages request body and headers for one stream
/// request (upstream `stream` + `buildParams` + `createClient`). Pure: no
/// HTTP, no I/O.
///
/// `cfg.base_url` is the provider endpoint (not consumed here); the
/// OpenRouter compat default checks `model.base_url` like upstream.
/// `cfg.api_key` is the credential fallback for `options.stream.api_key`.
pub fn build_request(
    model: &Model,
    cfg: &ProviderConfig,
    ctx: &TranscriptContext,
    options: &AnthropicOptions,
) -> Result<RequestAssembly, String> {
    // Preserve the pure builder's historical params + computed-header API.
    // The streaming path starts with client headers instead: callbacks can
    // replace/remove betas before the SDK generates request-time headers.
    let mut assembly = prepare_request(model, cfg, ctx, options)?;
    if let Some(betas) = assembly.body.get("betas").and_then(Value::as_array) {
        let betas: Vec<_> = betas.iter().filter_map(Value::as_str).collect();
        if !betas.is_empty() {
            set_header(&mut assembly.headers, "anthropic-beta", &betas.join(","));
        }
    }
    Ok(assembly)
}

/// Build SDK params and client-level headers, before process-local callbacks.
pub(super) fn prepare_request(
    model: &Model,
    cfg: &ProviderConfig,
    ctx: &TranscriptContext,
    options: &AnthropicOptions,
) -> Result<RequestAssembly, String> {
    let compat = get_anthropic_compat(model);
    // Upstream `stream` resolves the transcript once up front (line 517).
    let normalized =
        resolve_transcript(ctx.clone(), Some(compat.supports_mid_convo_system_messages));

    let copilot = model.provider == "github-copilot";
    let api_key = resolve_api_key(model, cfg, options)?;
    let is_oauth = !copilot && api_key.as_deref().is_some_and(is_oauth_token);

    let cache_retention =
        resolve_cache_retention(options.stream.cache_retention, options.stream.env.as_ref());
    // Upstream: cacheSessionId = retention !== "none" ? options.sessionId : undefined.
    let cache_session_id = (cache_retention != CacheRetention::None)
        .then(|| options.stream.session_id.clone())
        .flatten();

    let (body, _) = build_params(model, &normalized, options, is_oauth, &compat)?;
    let headers = build_headers(
        model,
        options,
        api_key.as_deref(),
        is_oauth,
        copilot,
        &normalized,
        &compat,
        cache_session_id.as_deref(),
        &[],
    );
    Ok(RequestAssembly { body, headers })
}

/// JSON-representable object spread used by upstream `{ ...nextParams }`.
/// Rust/serde strings cannot hold isolated UTF-16 surrogates; a top-level
/// non-BMP string is rejected rather than silently corrupting its indexed values.
pub(super) fn spread_params(value: Value) -> Result<Map<String, Value>, String> {
    match value {
        Value::Object(object) => Ok(object),
        Value::Array(values) => Ok(values
            .into_iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value))
            .collect()),
        Value::String(text) => text
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| {
                char::from_u32(u32::from(unit))
                    .map(|ch| (index.to_string(), Value::String(ch.to_string())))
                    .ok_or_else(|| "Anthropic payload string spread contains an isolated UTF-16 surrogate; use an object payload".to_string())
            })
            .collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(Map::new()),
    }
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// SDK betas?.toString() / Headers string coercion, for JSON payload values.
fn header_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                if value.is_null() {
                    String::new()
                } else {
                    header_string(value)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
        _ => value.to_string(),
    }
}

/// SDK 0.124.0 `Messages.create`: output_format migration and header-only
/// fields. Run AFTER onPayload; preflight errors must not be HTTP-retried.
/// Unlike the historical pure builder, this body is the actual wire body.
pub(super) fn sdk_request(assembly: &RequestAssembly) -> Result<RequestAssembly, String> {
    let mut body = spread_params(assembly.body.clone())?;
    if body.get("output_format").is_some_and(json_truthy) {
        if body
            .get("output_config")
            .and_then(|v| v.get("format"))
            .is_some_and(json_truthy)
        {
            return Err("Both output_format and output_config.format were provided. Please use only output_config.format (output_format is deprecated).".into());
        }
        let format = body
            .shift_remove("output_format")
            .expect("truthy format exists");
        let mut config = spread_params(body.get("output_config").cloned().unwrap_or(Value::Null))?;
        config.insert("format".into(), format);
        body.insert("output_config".into(), Value::Object(config));
    }
    let mut headers = assembly.headers.clone();
    for (field, header) in [
        ("betas", "anthropic-beta"),
        ("user_profile_id", "anthropic-user-profile-id"),
        ("workspace_id", "anthropic-workspace-id"),
    ] {
        if let Some(value) = body.shift_remove(field).filter(|value| !value.is_null()) {
            // [] deliberately produces an empty header, not omission.
            set_header(&mut headers, header, &header_string(&value));
        }
    }
    Ok(RequestAssembly {
        body: Value::Object(body),
        headers,
    })
}

// ---- request body (upstream buildParams, anthropic-messages.ts:1035-1205) ----

fn build_params(
    model: &Model,
    context: &TranscriptContext,
    options: &AnthropicOptions,
    is_oauth: bool,
    compat: &AnthropicCompat,
) -> Result<(Value, Vec<String>), String> {
    let cache_control = get_cache_control(
        compat,
        resolve_cache_retention(options.stream.cache_retention, options.stream.env.as_ref()),
    );
    let initial_system_message = get_initial_system_message(context.messages());
    let initial_system_text = initial_system_message
        .map(get_system_message_text)
        .unwrap_or_default();
    let transformed = transform_messages(model, context.messages(), &|id, _source| {
        normalize_tool_call_id(id)
    });
    let conversation_messages = if initial_system_message.is_some() {
        &transformed[1..]
    } else {
        &transformed[..]
    };
    let initial_tools = initial_system_message
        .and_then(|message| message.tools_added.clone())
        .unwrap_or_default();
    // Native tool changes keep the request-level tool list fixed and define
    // every later tool by value in a `tool_addition` block, which also
    // expresses same-name redefinitions (v1.0.0). Anthropic rejects an
    // all-deferred tool list, so there must be an initial active tool to
    // anchor the placeholder.
    let native_tool_changes = compat.supports_mid_convo_system_messages
        && compat.supports_mid_convo_tool_changes
        && !initial_tools.is_empty();
    let managed_provider = compat
        .supports_mid_convo_effort
        .then_some(model.provider.as_str());
    // Converts tool definitions for native `tool_addition` blocks; `None`
    // when tool changes are not native.
    let convert_tool_definitions: Option<ConvertToolDefinitions<'_>> = if native_tool_changes {
        Some(Box::new(move |tools: &[Tool]| {
            convert_tools(tools, is_oauth, compat, None)
        }))
    } else {
        None
    };
    let converted = convert_messages(
        conversation_messages,
        is_oauth,
        cache_control.as_ref(),
        compat.allow_empty_signature,
        managed_provider,
        convert_tool_definitions.as_deref(),
    )?;
    let active_effort = options.effort.map_or("high", AnthropicEffort::as_str);
    let beta_features = get_beta_features(
        model,
        context,
        is_oauth,
        native_tool_changes,
        options,
        compat,
    );

    let mut params = Map::new();
    params.insert("model".into(), Value::from(model.id.as_str()));
    params.insert(
        "messages".into(),
        if managed_provider.is_some() {
            Value::Array(insert_thinking_level_messages(converted, active_effort))
        } else {
            Value::Array(converted.messages)
        },
    );
    params.insert(
        "max_tokens".into(),
        json!(options.stream.max_tokens.unwrap_or(model.max_tokens)),
    );
    params.insert("stream".into(), Value::from(true));
    if !beta_features.is_empty() {
        params.insert("betas".into(), json!(beta_features));
    }

    // OAuth requests must include the Claude Code identity (lines 1077-1101).
    if is_oauth {
        let mut system = vec![system_text_block(
            CLAUDE_CODE_IDENTITY_PROMPT,
            cache_control.as_ref(),
        )];
        if !initial_system_text.is_empty() {
            system.push(system_text_block(
                &initial_system_text,
                cache_control.as_ref(),
            ));
        }
        params.insert("system".into(), Value::Array(system));
    } else if !initial_system_text.is_empty() {
        params.insert(
            "system".into(),
            Value::Array(vec![system_text_block(
                &initial_system_text,
                cache_control.as_ref(),
            )]),
        );
    }

    // Temperature is incompatible with extended thinking and unsupported on
    // Claude Opus 4.7+ (lines 1104-1113).
    if let Some(temperature) = options.stream.temperature {
        if options.thinking_enabled != Some(true)
            && managed_provider.is_none()
            && compat.supports_temperature
        {
            params.insert("temperature".into(), json!(temperature));
        }
    }

    let tool_cache_control = if compat.supports_cache_control_on_tools {
        cache_control.clone()
    } else {
        None
    };
    if native_tool_changes {
        // Initial tools stay active with the cache breakpoint on the last one,
        // followed by the placeholder. The list never changes afterwards: later
        // tools are defined by value in `tool_addition` blocks and withdrawn by
        // `tool_removal`, so the cached prefix survives every tool change
        // (lines 1203-1220).
        let mut tools = convert_tools(
            &initial_tools,
            is_oauth,
            compat,
            tool_cache_control.as_ref(),
        )?;
        tools.push(deferred_tool_placeholder());
        params.insert("tools".into(), Value::Array(tools));
    } else {
        let tools = get_current_tools(context.messages());
        if !tools.is_empty() {
            params.insert(
                "tools".into(),
                Value::Array(convert_tools(
                    &tools,
                    is_oauth,
                    compat,
                    tool_cache_control.as_ref(),
                )?),
            );
        }
    }

    // Managed effort models always use adaptive thinking so prefix mismatches
    // can be dropped instead of surfacing as persistent 400 responses
    // (lines 1151-1182).
    if compat.supports_mid_convo_effort {
        params.insert(
            "thinking".into(),
            json!({
                "type": "adaptive",
                "display": options.thinking_display.map_or("summarized", AnthropicThinkingDisplay::as_str),
                "block_binding": {"prefix_mismatch_behavior": "drop_block"}
            }),
        );
        params.insert("output_config".into(), json!({"effort": "high"}));
    } else if model.reasoning {
        if options.thinking_enabled == Some(true) {
            // Default to "summarized" so Opus 4.7 and Mythos Preview behave
            // like older Claude 4 models (lines 1162-1178).
            let display = options
                .thinking_display
                .map_or("summarized", AnthropicThinkingDisplay::as_str);
            if compat.force_adaptive_thinking == Some(true) {
                params.insert(
                    "thinking".into(),
                    json!({"type": "adaptive", "display": display}),
                );
                if let Some(effort) = options.effort {
                    params.insert("output_config".into(), json!({"effort": effort.as_str()}));
                }
            } else {
                // `options.thinkingBudgetTokens || 1024` (line 1175).
                let budget = match options.thinking_budget_tokens {
                    Some(0) | None => 1024,
                    Some(budget) => budget,
                };
                params.insert(
                    "thinking".into(),
                    json!({"type": "enabled", "budget_tokens": budget, "display": display}),
                );
            }
        } else if options.thinking_enabled == Some(false)
            // `model.thinkingLevelMap?.off !== null` (line 1179): only an
            // explicit null mapping omits the disabled param.
            && !matches!(
                model.thinking_level_map.as_ref().and_then(|map| map.get("off")),
                Some(None)
            )
        {
            params.insert("thinking".into(), json!({"type": "disabled"}));
        }
    }

    if let Some(metadata) = &options.stream.metadata {
        if let Some(Value::String(user_id)) = metadata.get("user_id") {
            params.insert("metadata".into(), json!({"user_id": user_id}));
        }
    }

    if let Some(choice) = &options.tool_choice {
        params.insert("tool_choice".into(), choice.to_wire());
    }

    if !compat.allowed_fallback_models.is_empty() {
        params.insert(
            "fallbacks".into(),
            Value::Array(
                compat
                    .allowed_fallback_models
                    .iter()
                    .map(|model_id| json!({"model": model_id}))
                    .collect(),
            ),
        );
    }

    Ok((Value::Object(params), beta_features))
}

/// Upstream `isAnthropicEffort` (lines 1430-1432).
fn is_anthropic_effort(value: &str) -> bool {
    AnthropicEffort::from_str_opt(value).is_some()
}

/// Upstream `insertThinkingLevelMessages` (lines 1434-1448): the historical
/// effort markers reconstruct the exact prefix; the current marker appends.
fn insert_thinking_level_messages(
    converted: ConvertedAnthropicMessages,
    active_effort: &str,
) -> Vec<Value> {
    let mut messages =
        Vec::with_capacity(converted.messages.len() + converted.assistant_levels.len() + 1);
    for (index, message) in converted.messages.into_iter().enumerate() {
        if let Some(level) = converted.assistant_levels.get(&index) {
            messages.push(json!({
                "role": "system",
                "content": [],
                "output_config": {"effort": level}
            }));
        }
        messages.push(message);
    }
    messages.push(json!({
        "role": "system",
        "content": [],
        "output_config": {"effort": active_effort}
    }));
    messages
}

// ---- message conversion (upstream convertMessages, lines 1226-1428) ----

/// Upstream `ConvertedAnthropicMessages` (lines 1221-1224).
struct ConvertedAnthropicMessages {
    messages: Vec<Value>,
    assistant_levels: HashMap<usize, String>,
}

/// Upstream `convertContentBlocks` (lines 128-175): text-only content
/// collapses into a string; images produce a block array, with a placeholder
/// text block when there is no text at all.
fn convert_content_blocks(content: &[TextOrImageBlock]) -> Value {
    let has_images = content
        .iter()
        .any(|block| matches!(block, TextOrImageBlock::Image(_)));
    if !has_images {
        let text: Vec<&str> = content
            .iter()
            .map(|block| match block {
                TextOrImageBlock::Text(text) => text.text.as_str(),
                TextOrImageBlock::Image(_) => "",
            })
            .collect();
        return Value::String(text.join("\n"));
    }
    let mut blocks: Vec<Value> = content
        .iter()
        .map(|block| match block {
            TextOrImageBlock::Text(text) => json!({"type": "text", "text": text.text}),
            TextOrImageBlock::Image(image) => json!({
                "type": "image",
                "source": {"type": "base64", "media_type": image.mime_type, "data": image.data}
            }),
        })
        .collect();
    let has_text = blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("text"));
    if !has_text {
        blocks.insert(
            0,
            json!({"type": "text", "text": IMAGE_ONLY_TOOL_RESULT_PLACEHOLDER}),
        );
    }
    Value::Array(blocks)
}

/// Upstream `convertToolResult` (lines 1212-1219).
fn convert_tool_result(message: &ToolResultMessage) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": message.tool_call_id,
        "content": convert_content_blocks(&message.content),
        "is_error": message.is_error
    })
}

/// User block-array conversion (lines 1280-1308): images pass through,
/// whitespace-only text blocks are dropped, and an empty result drops the
/// whole message.
fn convert_user_blocks(blocks: &[TextOrImageBlock]) -> Option<Vec<Value>> {
    let converted: Vec<Value> = blocks
        .iter()
        .filter_map(|block| match block {
            TextOrImageBlock::Text(text) => {
                (!text.text.trim().is_empty()).then(|| json!({"type": "text", "text": text.text}))
            }
            TextOrImageBlock::Image(image) => Some(json!({
                "type": "image",
                "source": {"type": "base64", "media_type": image.mime_type, "data": image.data}
            })),
        })
        .collect();
    (!converted.is_empty()).then_some(converted)
}

/// Assistant block conversion (lines 1309-1363): text, thinking replay with
/// full-fidelity signatures, and tool_use with CC naming for OAuth.
fn convert_assistant_blocks(
    content: &[AssistantBlock],
    is_oauth: bool,
    allow_empty_signature: bool,
) -> Vec<Value> {
    let mut blocks: Vec<Value> = Vec::new();
    for block in content {
        match block {
            AssistantBlock::Text(text) => {
                if text.text.trim().is_empty() {
                    continue;
                }
                blocks.push(json!({"type": "text", "text": text.text}));
            }
            AssistantBlock::Thinking(thinking) => {
                // Redacted thinking: pass the opaque payload back as
                // redacted_thinking (lines 1321-1328).
                if thinking.redacted == Some(true) {
                    let mut redacted = json!({"type": "redacted_thinking"});
                    if let Some(data) = &thinking.thinking_signature {
                        redacted["data"] = json!(data);
                    }
                    blocks.push(redacted);
                    continue;
                }
                let signature = thinking.thinking_signature.as_deref().unwrap_or("");
                let has_signature = !signature.trim().is_empty();
                if thinking.thinking.trim().is_empty() && !has_signature {
                    continue;
                }
                if !has_signature {
                    // Missing/empty signature (e.g. from an aborted stream):
                    // convert to plain text unless the provider accepts empty
                    // signatures (lines 1332-1347).
                    blocks.push(if allow_empty_signature {
                        json!({"type": "thinking", "thinking": thinking.thinking, "signature": ""})
                    } else {
                        json!({"type": "text", "text": thinking.thinking})
                    });
                } else {
                    blocks.push(json!({
                        "type": "thinking",
                        "thinking": thinking.thinking,
                        "signature": signature
                    }));
                }
            }
            AssistantBlock::ToolCall(call) => {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": if is_oauth { to_claude_code_name(&call.name) } else { call.name.clone() },
                    "input": if call.arguments.is_null() { json!({}) } else { call.arguments.clone() }
                }));
            }
        }
    }
    blocks
}

/// Upstream `convertMessages` (lines 1226-1428). `convert_tool_definitions`
/// (v1.0.0) converts tool definitions for native `tool_addition` blocks;
/// `None` when tool changes are not native.
fn convert_messages(
    messages: &[Message],
    is_oauth: bool,
    cache_control: Option<&Value>,
    allow_empty_signature: bool,
    managed_provider: Option<&str>,
    convert_tool_definitions: Option<&ConvertToolDefinitionsFn>,
) -> Result<ConvertedAnthropicMessages, String> {
    let mut params: Vec<Value> = Vec::new();
    let mut assistant_levels: HashMap<usize, String> = HashMap::new();
    // Later system messages are held back and emitted directly before the next
    // assistant message (or at the end of the transcript): tool_result blocks
    // must immediately follow their tool_use (lines 1236-1245).
    let mut pending_system_messages: Vec<Value> = Vec::new();

    let mut index = 0usize;
    while index < messages.len() {
        match &messages[index] {
            Message::System(system) => {
                let text = render_system_message_update(system);
                let mut blocks: Vec<Value> = Vec::new();
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                if let Some(convert_tool_definitions) = convert_tool_definitions {
                    let added: Vec<Tool> = system.tools_added.clone().unwrap_or_default();
                    let redefined: HashSet<&str> =
                        added.iter().map(|tool| tool.name.as_str()).collect();
                    for tool in system.tools_removed.iter().flatten() {
                        // A new definition under the same name replaces the
                        // old one, so no removal is needed (v1.0.0).
                        if redefined.contains(tool.name.as_str()) {
                            continue;
                        }
                        blocks.push(json!({
                            "type": "tool_removal",
                            "tool": {"type": "tool_reference", "name": if is_oauth { to_claude_code_name(&tool.name) } else { tool.name.clone() }}
                        }));
                    }
                    for definition in convert_tool_definitions(&added)? {
                        blocks.push(json!({
                            "type": "tool_addition",
                            "tool": {"type": "tool_definition", "definition": definition}
                        }));
                    }
                }
                if !blocks.is_empty() {
                    pending_system_messages.push(json!({"role": "system", "content": blocks}));
                }
                index += 1;
            }
            Message::User(user) => match &user.content {
                StringOrBlocks::Text(text) => {
                    if !text.trim().is_empty() {
                        params.push(json!({"role": "user", "content": text}));
                    }
                    index += 1;
                }
                StringOrBlocks::Blocks(blocks) => {
                    if let Some(converted) = convert_user_blocks(blocks) {
                        params.push(json!({"role": "user", "content": converted}));
                    }
                    index += 1;
                }
            },
            Message::Assistant(assistant) => {
                params.append(&mut pending_system_messages);
                let blocks =
                    convert_assistant_blocks(&assistant.content, is_oauth, allow_empty_signature);
                if !blocks.is_empty() {
                    let message_index = params.len();
                    params.push(json!({"role": "assistant", "content": blocks}));
                    if Some(assistant.provider.as_str()) == managed_provider
                        && assistant.api == "anthropic-messages"
                        && assistant
                            .provider_thinking_level
                            .as_deref()
                            .is_some_and(is_anthropic_effort)
                    {
                        assistant_levels.insert(
                            message_index,
                            assistant
                                .provider_thinking_level
                                .clone()
                                .unwrap_or_default(),
                        );
                    }
                }
                index += 1;
            }
            Message::ToolResult(_) => {
                // Collect all consecutive toolResult messages, needed for z.ai
                // Anthropic endpoints (lines 1378-1394).
                let mut tool_results: Vec<Value> = Vec::new();
                let mut j = index;
                while j < messages.len() {
                    let Message::ToolResult(result) = &messages[j] else {
                        break;
                    };
                    tool_results.push(convert_tool_result(result));
                    j += 1;
                }
                params.push(json!({"role": "user", "content": tool_results}));
                index = j;
            }
        }
    }

    params.append(&mut pending_system_messages);

    // Cache the conversation history on the last user or system message
    // (lines 1399-1425).
    if let Some(marker) = cache_control {
        add_cache_control_to_last_message(&mut params, marker);
    }

    Ok(ConvertedAnthropicMessages {
        messages: params,
        assistant_levels,
    })
}

/// Upstream lines 1399-1425.
fn add_cache_control_to_last_message(params: &mut [Value], cache_control: &Value) {
    let Some(last) = params.last_mut() else {
        return;
    };
    if !matches!(
        last.get("role").and_then(Value::as_str),
        Some("user") | Some("system")
    ) {
        return;
    }
    match last.get("content") {
        Some(Value::Array(blocks)) => {
            let Some(last_block) = blocks.last() else {
                return;
            };
            if matches!(
                last_block.get("type").and_then(Value::as_str),
                Some("text")
                    | Some("image")
                    | Some("tool_result")
                    | Some("tool_addition")
                    | Some("tool_removal")
            ) {
                if let Some(object) = last.get_mut("content").and_then(Value::as_array_mut) {
                    if let Some(last_block) = object.last_mut() {
                        if let Some(block_object) = last_block.as_object_mut() {
                            block_object.insert("cache_control".into(), cache_control.clone());
                        }
                    }
                }
            }
        }
        Some(Value::String(text)) => {
            let text = text.clone();
            last["content"] = json!([{
                "type": "text",
                "text": text,
                "cache_control": cache_control
            }]);
        }
        _ => {}
    }
}

// Keywords Anthropic strict tool use rejects with a 400 for the whole
// request (upstream anthropic-messages.ts, the non-strict-fallback delta):
// https://platform.claude.com/docs/en/build-with-claude/structured-outputs#json-schema-limitations
const ANTHROPIC_STRICT_UNSUPPORTED_KEYWORDS: [&str; 11] = [
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "maxItems",
    "uniqueItems",
    "minContains",
    "maxContains",
    "minProperties",
    "maxProperties",
];
const ANTHROPIC_STRICT_STRING_FORMATS: [&str; 10] = [
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "uri",
    "ipv4",
    "ipv6",
    "uuid",
];

/// Upstream `isAnthropicStrictUnsupportedKeyword`: the provider-specific
/// strict-mode keyword rejection passed to
/// `resolveJsonSchemaStrictSampling`.
fn is_anthropic_strict_unsupported_keyword(key: &str, value: &Value) -> bool {
    if ANTHROPIC_STRICT_UNSUPPORTED_KEYWORDS.contains(&key) {
        return true;
    }
    if key == "minItems" {
        return !matches!(value.as_u64(), Some(0) | Some(1));
    }
    if key == "format" {
        return match value.as_str() {
            Some(format) => !ANTHROPIC_STRICT_STRING_FORMATS.contains(&format),
            None => true,
        };
    }
    false
}

// ---- tools (upstream convertTools, lines 1457-1492) ----

fn convert_tools(
    tools: &[Tool],
    is_oauth: bool,
    compat: &AnthropicCompat,
    cache_control: Option<&Value>,
) -> Result<Vec<Value>, String> {
    let mut converted: Vec<Value> = Vec::with_capacity(tools.len());
    for (index, tool) in tools.iter().enumerate() {
        let strict = resolve_json_schema_strict_sampling(
            tool,
            compat.supports_strict_tools,
            Some(is_anthropic_strict_unsupported_keyword),
        )?;
        let parameters = if strict == Some(true) {
            make_strict_json_schema(
                &tool.parameters,
                Some(is_anthropic_strict_unsupported_keyword),
            )?
        } else {
            tool.parameters.clone()
        };
        // Legacy input schema (lines 1470-1481): the normalized object shape;
        // strict tools extend it with the full transformed schema.
        let mut input_schema = Map::new();
        if strict == Some(true) {
            if let Some(object) = parameters.as_object() {
                for (key, value) in object {
                    input_schema.insert(key.clone(), value.clone());
                }
            }
        }
        input_schema.insert("type".into(), json!("object"));
        let properties = parameters
            .get("properties")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or_else(|| json!({}));
        input_schema.insert("properties".into(), properties);
        let required = parameters
            .get("required")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or_else(|| json!([]));
        input_schema.insert("required".into(), required);

        let mut converted_tool = Map::new();
        converted_tool.insert(
            "name".into(),
            json!(if is_oauth {
                to_claude_code_name(&tool.name)
            } else {
                tool.name.clone()
            }),
        );
        converted_tool.insert("description".into(), json!(tool.description));
        if compat.supports_eager_tool_input_streaming {
            converted_tool.insert("eager_input_streaming".into(), json!(true));
        }
        if strict == Some(true) {
            converted_tool.insert("strict".into(), json!(true));
        }
        converted_tool.insert("input_schema".into(), Value::Object(input_schema));
        if let Some(marker) = cache_control {
            if index == tools.len() - 1 {
                converted_tool.insert("cache_control".into(), marker.clone());
            }
        }
        converted.push(Value::Object(converted_tool));
    }
    Ok(converted)
}

// ---- headers (upstream createClient, lines 910-989, + SDK-injected pairs) ----

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    match headers
        .iter_mut()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
    {
        Some(entry) => entry.1 = value.to_string(),
        None => headers.push((name.to_string(), value.to_string())),
    }
}

fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
}

#[allow(clippy::too_many_arguments)]
fn build_headers(
    model: &Model,
    options: &AnthropicOptions,
    api_key: Option<&str>,
    is_oauth: bool,
    copilot: bool,
    context: &TranscriptContext,
    compat: &AnthropicCompat,
    session_id: Option<&str>,
    betas: &[String],
) -> Vec<(String, String)> {
    // `mergeClientHeaders` base (lines 294-296).
    let mut headers: Vec<(String, String)> = vec![("User-Agent".to_string(), pi_user_agent())];
    set_header(&mut headers, "accept", "application/json");
    set_header(
        &mut headers,
        "anthropic-dangerous-direct-browser-access",
        "true",
    );
    if is_oauth {
        // Claude Code identity headers (lines 950-957); overrides the pi
        // User-Agent like the SDK's case-insensitive header merge.
        set_header(
            &mut headers,
            "user-agent",
            &format!("claude-cli/{CLAUDE_CODE_VERSION}"),
        );
        set_header(&mut headers, "x-app", "cli");
    }
    if copilot {
        // Copilot dynamic headers (lines 555-561).
        for (name, value) in build_copilot_dynamic_headers(context.messages()) {
            set_header(&mut headers, &name, &value);
        }
    } else if !is_oauth {
        // Session affinity only on the API-key path (lines 963-969).
        if let Some(session_id) = session_id {
            if compat.send_session_affinity_headers {
                let name = if compat.session_affinity_is_openrouter {
                    "x-session-id"
                } else {
                    "x-session-affinity"
                };
                set_header(&mut headers, name, session_id);
            }
        }
    }
    // Model headers; a None value (upstream null) suppresses a default,
    // like the options-level merge below.
    for (name, value) in model.headers.iter().flatten() {
        match value {
            Some(value) => set_header(&mut headers, name, value),
            None => remove_header(&mut headers, name),
        }
    }
    // Caller headers last; a None value (upstream null) suppresses a default.
    if let Some(option_headers) = &options.stream.headers {
        for (name, value) in option_headers {
            match value {
                Some(value) => set_header(&mut headers, name, value),
                None => remove_header(&mut headers, name),
            }
        }
    }
    // SDK-injected auth pair: `authToken` sends a bearer header, `apiKey`
    // sends `x-api-key` (copilot lines 918-937, OAuth 940-961, key 963-988).
    if let Some(api_key) = api_key {
        if copilot || is_oauth {
            set_header(&mut headers, "Authorization", &format!("Bearer {api_key}"));
        } else {
            set_header(&mut headers, "x-api-key", api_key);
        }
    }
    set_header(&mut headers, "anthropic-version", ANTHROPIC_VERSION_HEADER);
    // The SDK emits the betas list as the request-time `anthropic-beta`
    // header, overriding any default-header value.
    if !betas.is_empty() {
        set_header(&mut headers, "anthropic-beta", &betas.join(","));
    }
    headers
}

// ---- copilot dynamic headers (upstream api/github-copilot-headers.ts) ----

/// Upstream `inferCopilotInitiator` (github-copilot-headers.ts:5-8).
fn infer_copilot_initiator(messages: &[Message]) -> &'static str {
    match messages.last() {
        Some(Message::User(_)) | None => "user",
        Some(_) => "agent",
    }
}

/// Upstream `hasCopilotVisionInput` (github-copilot-headers.ts:11-21).
fn has_copilot_vision_input(messages: &[Message]) -> bool {
    messages.iter().any(|message| match message {
        Message::User(user) => matches!(&user.content,
            StringOrBlocks::Blocks(blocks) if blocks.iter().any(|block| matches!(block, TextOrImageBlock::Image(_)))),
        Message::ToolResult(result) => result
            .content
            .iter()
            .any(|block| matches!(block, TextOrImageBlock::Image(_))),
        _ => false,
    })
}

/// Upstream `buildCopilotDynamicHeaders` (github-copilot-headers.ts:23-37).
fn build_copilot_dynamic_headers(messages: &[Message]) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "X-Initiator".to_string(),
            infer_copilot_initiator(messages).to_string(),
        ),
        (
            "Openai-Intent".to_string(),
            "conversation-edits".to_string(),
        ),
    ];
    if has_copilot_vision_input(messages) {
        headers.push(("Copilot-Vision-Request".to_string(), "true".to_string()));
    }
    headers
}

// ---- streamSimple option shaping (upstream lines 858-904 + simple-options.ts) ----

/// Upstream `mapThinkingLevelToEffort` (lines 838-856): the model's
/// thinkingLevelMap wins when it maps the level to a valid effort string; the
/// fallback switch sends minimal/low to "low", medium to "medium", and
/// everything else to "high".
fn map_thinking_level_to_effort(model: &Model, level: ThinkingLevel) -> AnthropicEffort {
    let key = match level {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    };
    let mapped = model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(key))
        .and_then(|value| value.as_deref())
        .and_then(AnthropicEffort::from_str_opt);
    if let Some(effort) = mapped {
        return effort;
    }
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => AnthropicEffort::Low,
        ThinkingLevel::Medium => AnthropicEffort::Medium,
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => AnthropicEffort::High,
    }
}

/// Upstream `adjustMaxTokensForThinking` (api/simple-options.ts:79-95).
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

/// Port of upstream `streamSimple`'s option shaping (anthropic-messages.ts
/// lines 858-904 plus `api/simple-options.ts`): the `buildBaseOptions` merge,
/// the reasoning-level mapping to thinking options (adaptive effort for
/// `forceAdaptiveThinking` models, budget-based thinking otherwise), and the
/// `off` mapping to `thinkingEnabled: false`.
pub fn options_from_simple(
    model: &Model,
    ctx: &TranscriptContext,
    options: &SimpleStreamOptions,
) -> AnthropicOptions {
    let compat = get_anthropic_compat(model);
    // Upstream `buildBaseOptions` (api/simple-options.ts:21-52).
    let sampling_params =
        if model.sampling_params.is_some() || options.stream.sampling_params.is_some() {
            let mut merged = model.sampling_params.clone().unwrap_or_default();
            for (key, value) in options.stream.sampling_params.iter().flatten() {
                merged.insert(key.clone(), value.clone());
            }
            Some(merged)
        } else {
            None
        };
    let base_max_tokens = clamp_max_tokens_to_context(
        model,
        ctx,
        options.stream.max_tokens.unwrap_or(model.max_tokens),
    );
    let mut result = AnthropicOptions {
        stream: StreamOptions {
            callbacks: options.stream.callbacks.clone(),
            signal: options.stream.signal.clone(),
            temperature: options.stream.temperature,
            sampling_params,
            max_tokens: Some(base_max_tokens),
            api_key: options.stream.api_key.clone(),
            env: options.stream.env.clone(),
            headers: options.stream.headers.clone(),
            timeout_ms: options.stream.timeout_ms,
            max_retries: options.stream.max_retries,
            max_retry_delay_ms: options.stream.max_retry_delay_ms,
            transport: options.stream.transport,
            cache_retention: options.stream.cache_retention,
            session_id: options.stream.session_id.clone(),
            websocket_connect_timeout_ms: options.stream.websocket_connect_timeout_ms,
            metadata: options.stream.metadata.clone(),
        },
        tool_choice: options.tool_choice.map(AnthropicToolChoice::from),
        ..AnthropicOptions::default()
    };

    match options.reasoning {
        // Upstream lines 869-874: no reasoning -> thinking explicitly off.
        None => result.thinking_enabled = Some(false),
        // Adaptive models use effort levels (lines 877-885).
        Some(level) if compat.force_adaptive_thinking == Some(true) => {
            result.thinking_enabled = Some(true);
            result.effort = Some(map_thinking_level_to_effort(model, level));
        }
        // Older models use budget-based thinking (lines 887-903).
        Some(level) => {
            let (adjusted_max_tokens, thinking_budget) = adjust_max_tokens_for_thinking(
                Some(base_max_tokens),
                model.max_tokens,
                level,
                options.thinking_budgets.as_ref(),
            );
            let max_tokens = clamp_max_tokens_to_context(model, ctx, adjusted_max_tokens);
            result.stream.max_tokens = Some(max_tokens);
            result.thinking_enabled = Some(true);
            result.thinking_budget_tokens =
                Some(thinking_budget.min(max_tokens.saturating_sub(MIN_ANSWER_TOKENS)));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::pi_user_agent as ua;
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::content::{ImageContent, TextContent, ThinkingContent, ToolCall};
    use crate::ai::types::message::{
        AssistantMessage, SystemMessage, ToolResultMessage, UserMessage,
    };
    use crate::ai::types::primitives::{ModelCost, StopReason, Usage, UsageCost};
    use crate::ai::types::tool::{ConstrainedSampling, JsonSchemaSampling, Strict};
    use crate::ai::types::ModelInput;
    use serde_json::json;
    use std::collections::BTreeMap;

    const TS: i64 = 1758240000000;

    #[test]
    fn sdk_request_preserves_surviving_and_overwritten_field_order() {
        for (input, expected) in [
            (
                r#"{"betas":[],"z":1,"user_profile_id":"u","a":2,"workspace_id":"w","b":3}"#,
                r#"{"z":1,"a":2,"b":3}"#,
            ),
            (
                r#"{"z":1,"output_format":{"type":"json_schema"},"a":2,"output_config":{"effort":"high"},"b":3}"#,
                r#"{"z":1,"a":2,"output_config":{"effort":"high","format":{"type":"json_schema"}},"b":3}"#,
            ),
            (
                r#"{"z":1,"output_format":{"type":"json_schema"},"a":2,"b":3}"#,
                r#"{"z":1,"a":2,"b":3,"output_config":{"format":{"type":"json_schema"}}}"#,
            ),
        ] {
            let assembly = RequestAssembly {
                body: serde_json::from_str(input).unwrap(),
                headers: vec![],
            };
            let actual = sdk_request(&assembly).unwrap();
            assert_eq!(actual.body.to_string(), expected);
            assert_eq!(
                assembly.body.to_string(),
                input,
                "input must stay untouched"
            );
        }
    }

    // ---- fixtures ----

    fn make_model(compat: Value) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "claude-test".to_string(),
            name: "Claude Test".to_string(),
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: ModelCost::default(),
            context_window: 200000,
            max_tokens: 32000,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: Some(compat),
        }
    }

    fn managed_model() -> Model {
        let mut model = make_model(json!({
            "forceAdaptiveThinking": true,
            "supportsMidConvoEffort": true
        }));
        model.id = "claude-fable-5-1".to_string();
        model.thinking_level_map = Some(BTreeMap::from([
            ("off".to_string(), None),
            ("minimal".to_string(), Some("low".to_string())),
            ("low".to_string(), Some("low".to_string())),
            ("medium".to_string(), Some("medium".to_string())),
            ("high".to_string(), Some("high".to_string())),
            ("max".to_string(), Some("max".to_string())),
        ]));
        model
    }

    fn cfg() -> ProviderConfig {
        cfg_with_key("test-key")
    }

    fn cfg_with_key(key: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: "https://api.anthropic.com".to_string(),
            api_key: key.to_string(),
            max_tokens: 32000,
        }
    }

    fn opts() -> AnthropicOptions {
        AnthropicOptions::default()
    }

    fn ctx_of(messages: Vec<Message>) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: None,
            messages,
            tools: None,
        })
    }

    fn prompt_ctx(prompt: &str, messages: Vec<Message>) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: Some(prompt.to_string()),
            messages,
            tools: None,
        })
    }

    fn build(
        model: &Model,
        ctx: &TranscriptContext,
        options: &AnthropicOptions,
    ) -> RequestAssembly {
        build_request(model, &cfg(), ctx, options).unwrap()
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

    fn system_msg(content: &str, ts: i64) -> Message {
        Message::System(SystemMessage {
            content: StringOrBlocks::Text(content.to_string()),
            sections: None,
            tools_added: None,
            tools_removed: None,
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
        TextOrImageBlock::Image(ImageContent {
            data: "aGVsbG8=".to_string(),
            mime_type: "image/png".to_string(),
        })
    }

    fn thinking(text: &str, signature: Option<&str>) -> AssistantBlock {
        AssistantBlock::Thinking(ThinkingContent {
            thinking: text.to_string(),
            thinking_signature: signature.map(str::to_string),
            redacted: None,
        })
    }

    fn tool_call(id: &str, name: &str) -> AssistantBlock {
        AssistantBlock::ToolCall(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: json!({"command": "ls"}),
            thought_signature: None,
            namespace: None,
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

    fn assistant_msg(
        provider: &str,
        api: &str,
        model_id: &str,
        content: Vec<AssistantBlock>,
        level: Option<&str>,
    ) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            api: api.to_string(),
            provider: provider.to_string(),
            model: model_id.to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: level.map(str::to_string),
            diagnostics: None,
            usage: empty_usage(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: TS,
        })
    }

    fn same_model_assistant(content: Vec<AssistantBlock>) -> Message {
        assistant_msg(
            "anthropic",
            "anthropic-messages",
            "claude-test",
            content,
            None,
        )
    }

    fn result_msg(call_id: &str, is_error: bool, content: Vec<TextOrImageBlock>) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: call_id.to_string(),
            tool_name: "bash".to_string(),
            content,
            details: None,
            usage: None,
            is_error,
            timestamp: TS,
        })
    }

    fn tool_decl(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: format!("Tool {name}"),
            parameters: json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            constrained_sampling: None,
        }
    }

    fn system_with_tools(content: &str, ts: i64, tools: Vec<Tool>) -> Message {
        Message::System(SystemMessage {
            content: StringOrBlocks::Text(content.to_string()),
            sections: None,
            tools_added: (!tools.is_empty()).then_some(tools),
            tools_removed: None,
            timestamp: ts,
        })
    }

    fn tools_ctx(tools: Vec<Tool>, messages: Vec<Message>) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: None,
            messages,
            tools: (!tools.is_empty()).then_some(tools),
        })
    }

    fn header<'a>(assembly: &'a RequestAssembly, name: &str) -> Option<&'a str> {
        assembly
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn betas(assembly: &RequestAssembly) -> Vec<String> {
        header(assembly, "anthropic-beta")
            .map(|value| value.split(',').map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn opt_headers(pairs: &[(&str, Option<&str>)]) -> ProviderHeaders {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.map(str::to_string)))
            .collect()
    }

    // ---- system (must-cover 1) ----

    #[test]
    fn system_prompt_replayed_and_mid_convo_messages_folded() {
        let model = make_model(json!({}));
        let ctx = prompt_ctx("Base.", vec![user("hi"), system_msg("Extra.", TS + 1)]);
        let assembly = build(&model, &ctx, &opts());

        assert_eq!(
            assembly.body["system"],
            json!([{
                "type": "text",
                "text": "Base.\n\nExtra.",
                "cache_control": {"type": "ephemeral"}
            }])
        );
        // The later system message is folded into the prompt, not replayed.
        // The default short retention marks the last user message (upstream
        // lines 1415-1423 convert string content to a marked block array).
        assert_eq!(
            assembly.body["messages"],
            json!([{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hi",
                    "cache_control": {"type": "ephemeral"}
                }]
            }])
        );
    }

    #[test]
    fn mid_convo_system_messages_kept_in_place_when_supported() {
        let mut model = make_model(json!({"supportsMidConvoSystemMessages": true}));
        model.reasoning = false;
        let ctx = prompt_ctx(
            "Base.",
            vec![
                user("hi"),
                system_msg("Extra.", TS + 1),
                same_model_assistant(vec![AssistantBlock::Text(TextContent {
                    text: "answer".to_string(),
                    text_signature: None,
                })]),
                user("ho"),
            ],
        );
        let assembly = build(&model, &ctx, &opts());

        // Held back until the next assistant message.
        assert_eq!(
            assembly.body["messages"],
            json!([
                {"role": "user", "content": "hi"},
                {"role": "system", "content": [{"type": "text", "text": "Extra."}]},
                {"role": "assistant", "content": [{"type": "text", "text": "answer"}]},
                {"role": "user", "content": [{
                    "type": "text",
                    "text": "ho",
                    "cache_control": {"type": "ephemeral"}
                }]},
            ])
        );
        // The trailing cache marker lands on the last user message.
        assert_eq!(
            assembly.body["messages"][3],
            json!({
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "ho",
                    "cache_control": {"type": "ephemeral"}
                }]
            })
        );
        assert_eq!(
            assembly.body["system"],
            json!([{"type": "text", "text": "Base.", "cache_control": {"type": "ephemeral"}}])
        );
    }

    #[test]
    fn cache_retention_controls_ttl_and_markers() {
        let ctx = prompt_ctx("Base.", vec![user("hi")]);

        // Long retention with default compat (supportsLongCacheRetention ?? true).
        let model = make_model(json!({}));
        let options = AnthropicOptions {
            stream: StreamOptions {
                cache_retention: Some(CacheRetention::Long),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert_eq!(
            assembly.body["system"][0]["cache_control"],
            json!({"type": "ephemeral", "ttl": "1h"})
        );

        // Long retention without support: marker without ttl.
        let model = make_model(json!({"supportsLongCacheRetention": false}));
        let assembly = build(&model, &ctx, &options);
        assert_eq!(
            assembly.body["system"][0]["cache_control"],
            json!({"type": "ephemeral"})
        );

        // Retention none: no markers anywhere.
        let model = make_model(json!({}));
        let options = AnthropicOptions {
            stream: StreamOptions {
                cache_retention: Some(CacheRetention::None),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert!(assembly.body["system"][0].get("cache_control").is_none());
        assert!(assembly.body["messages"][0]
            .get("content")
            .and_then(Value::as_str)
            .is_some());
    }

    // ---- messages (must-cover 2) ----

    #[test]
    fn user_content_text_images_and_whitespace_filtering() {
        let mut model = make_model(json!({}));
        model.input = vec![ModelInput::Text, ModelInput::Image];
        let ctx = ctx_of(vec![
            user("hello"),
            user_blocks(
                vec![text_block("   "), image_block(), text_block("desc")],
                TS + 1,
            ),
        ]);
        let assembly = build(&model, &ctx, &opts());

        assert_eq!(
            assembly.body["messages"][0],
            json!({"role": "user", "content": "hello"})
        );
        assert_eq!(
            assembly.body["messages"][1],
            json!({
                "role": "user",
                "content": [
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aGVsbG8="}},
                    {"type": "text", "text": "desc", "cache_control": {"type": "ephemeral"}}
                ]
            })
        );
    }

    #[test]
    fn assistant_thinking_replay_with_signatures() {
        let model = make_model(json!({}));
        let ctx = ctx_of(vec![
            user("first"),
            same_model_assistant(vec![
                thinking("reasoning", Some("sig1")),
                thinking("", Some("signed-thinking")),
                thinking("unsigned", None),
                AssistantBlock::Text(TextContent {
                    text: "answer".to_string(),
                    text_signature: None,
                }),
                tool_call("call_1", "bash"),
            ]),
            user("second"),
        ]);
        let assembly = build(&model, &ctx, &opts());

        assert_eq!(
            assembly.body["messages"][1],
            json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "reasoning", "signature": "sig1"},
                    {"type": "thinking", "thinking": "", "signature": "signed-thinking"},
                    {"type": "text", "text": "unsigned"},
                    {"type": "text", "text": "answer"},
                    {"type": "tool_use", "id": "call_1", "name": "bash", "input": {"command": "ls"}}
                ]
            })
        );
    }

    #[test]
    fn empty_signature_thinking_compat() {
        let ctx = ctx_of(vec![
            user("first"),
            same_model_assistant(vec![thinking("internal reasoning", Some(" "))]),
            user("second"),
        ]);

        // Default: converted to plain text.
        let model = make_model(json!({}));
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(
            assembly.body["messages"][1]["content"],
            json!([{"type": "text", "text": "internal reasoning"}])
        );

        // allowEmptySignature: preserved with an empty signature.
        let model = make_model(json!({"allowEmptySignature": true}));
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(
            assembly.body["messages"][1]["content"],
            json!([{"type": "thinking", "thinking": "internal reasoning", "signature": ""}])
        );
    }

    #[test]
    fn redacted_thinking_replays_opaque_payload() {
        let model = make_model(json!({}));
        let ctx = ctx_of(vec![
            user("first"),
            same_model_assistant(vec![AssistantBlock::Thinking(ThinkingContent {
                thinking: String::new(),
                thinking_signature: Some("ENCRYPTED".to_string()),
                redacted: Some(true),
            })]),
            user("second"),
        ]);
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(
            assembly.body["messages"][1]["content"],
            json!([{"type": "redacted_thinking", "data": "ENCRYPTED"}])
        );
    }

    #[test]
    fn tool_results_grouped_with_is_error_and_images() {
        let mut model = make_model(json!({}));
        model.input = vec![ModelInput::Text, ModelInput::Image];
        let ctx = ctx_of(vec![
            user("run"),
            same_model_assistant(vec![
                tool_call("call_1", "bash"),
                tool_call("call_2", "read"),
            ]),
            result_msg("call_1", false, vec![text_block("total 0")]),
            result_msg("call_2", true, vec![image_block()]),
            user("next"),
        ]);
        let assembly = build(&model, &ctx, &opts());

        // Consecutive tool results group into one user message.
        assert_eq!(
            assembly.body["messages"][2],
            json!({
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": "total 0", "is_error": false},
                    {"type": "tool_result", "tool_use_id": "call_2", "content": [
                        {"type": "text", "text": "(see attached image)"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aGVsbG8="}}
                    ], "is_error": true}
                ]
            })
        );
    }

    #[test]
    fn orphaned_tool_call_gets_synthetic_error_result() {
        let model = make_model(json!({}));
        let ctx = ctx_of(vec![
            user("run"),
            same_model_assistant(vec![tool_call("call_9", "bash")]),
            user("next"),
        ]);
        let assembly = build(&model, &ctx, &opts());

        assert_eq!(
            assembly.body["messages"][2],
            json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "call_9",
                    "content": "No result provided",
                    "is_error": true
                }]
            })
        );
    }

    #[test]
    fn cross_model_tool_call_ids_normalized_consistently() {
        let model = make_model(json!({}));
        let weird_id = "weird id|item#1";
        let ctx = ctx_of(vec![
            user("run"),
            assistant_msg(
                "other-provider",
                "openai-completions",
                "gpt-x",
                vec![tool_call(weird_id, "bash")],
                None,
            ),
            result_msg(weird_id, false, vec![text_block("ok")]),
        ]);
        let assembly = build(&model, &ctx, &opts());

        assert_eq!(
            assembly.body["messages"][1]["content"][0]["id"],
            json!("weird_id_item_1")
        );
        assert_eq!(
            assembly.body["messages"][2]["content"][0]["tool_use_id"],
            json!("weird_id_item_1")
        );
    }

    // ---- tools (must-cover 3) ----

    #[test]
    fn tools_legacy_input_schema_and_cache_control_on_last_tool() {
        let model = make_model(json!({}));
        let mut second = tool_decl("store");
        second.parameters = json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
            "title": "StoreInput"
        });
        let ctx = tools_ctx(vec![tool_decl("lookup"), second], vec![user("hi")]);
        let assembly = build(&model, &ctx, &opts());

        // Legacy shape only: extra schema keys are dropped.
        assert_eq!(
            assembly.body["tools"][0],
            json!({
                "name": "lookup",
                "description": "Tool lookup",
                "eager_input_streaming": true,
                "input_schema": {
                    "type": "object",
                    "properties": {"value": {"type": "string"}},
                    "required": ["value"]
                }
            })
        );
        // cache_control rides on the last tool only.
        assert_eq!(
            assembly.body["tools"][1],
            json!({
                "name": "store",
                "description": "Tool store",
                "eager_input_streaming": true,
                "input_schema": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                },
                "cache_control": {"type": "ephemeral"}
            })
        );

        // supportsCacheControlOnTools false: no markers on tools.
        let model = make_model(json!({"supportsCacheControlOnTools": false}));
        let assembly = build(&model, &ctx, &opts());
        assert!(assembly.body["tools"][1].get("cache_control").is_none());
    }

    #[test]
    fn eager_tool_input_streaming_compat() {
        let ctx = tools_ctx(vec![tool_decl("lookup")], vec![user("hi")]);

        // Default: per-tool eager_input_streaming, no beta header.
        let model = make_model(json!({"forceAdaptiveThinking": true}));
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(
            assembly.body["tools"][0]["eager_input_streaming"],
            json!(true)
        );
        assert_eq!(betas(&assembly), Vec::<String>::new());

        // Disabled: legacy fine-grained tool streaming beta instead.
        let model = make_model(
            json!({"forceAdaptiveThinking": true, "supportsEagerToolInputStreaming": false}),
        );
        let assembly = build(&model, &ctx, &opts());
        assert!(assembly.body["tools"][0]
            .get("eager_input_streaming")
            .is_none());
        assert_eq!(
            header(&assembly, "anthropic-beta"),
            Some("fine-grained-tool-streaming-2025-05-14")
        );

        // No tools: no legacy beta either.
        let ctx = ctx_of(vec![user("hi")]);
        let assembly = build(&model, &ctx, &opts());
        assert!(assembly.body.get("tools").is_none());
        assert_eq!(betas(&assembly), Vec::<String>::new());
    }

    #[test]
    fn native_tool_changes_define_later_tools_inline() {
        let model = make_model(json!({
            "supportsMidConvoSystemMessages": true,
            "supportsMidConvoToolChanges": true
        }));
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![
                system_with_tools("Base.", TS, vec![tool_decl("lookup")]),
                user("hi"),
                system_with_tools("More.", TS + 1, vec![tool_decl("store")]),
                same_model_assistant(vec![AssistantBlock::Text(TextContent {
                    text: "answer".to_string(),
                    text_signature: None,
                })]),
            ],
            tools: None,
        });
        let options = AnthropicOptions {
            stream: no_cache(),
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);

        // Initial tools stay active with the cache breakpoint on the last one,
        // followed by the placeholder. The list never changes afterwards: later
        // tools are defined by value in `tool_addition` blocks and withdrawn by
        // `tool_removal`, so the cached prefix survives every tool change
        // (upstream lines 1203-1220).
        assert_eq!(
            assembly.body["tools"],
            json!([
                {
                    "name": "lookup",
                    "description": "Tool lookup",
                    "eager_input_streaming": true,
                    "input_schema": {
                        "type": "object",
                        "properties": {"value": {"type": "string"}},
                        "required": ["value"]
                    }
                },
                {
                    "name": "__pi_deferred_placeholder__",
                    "description": "Reserved placeholder. Never available. Never call this.",
                    "input_schema": {"type": "object", "properties": {}, "required": []},
                    "defer_loading": true
                }
            ])
        );
        // The later system message carries the inline tool definition in place,
        // held back until the next assistant message.
        assert_eq!(
            assembly.body["messages"],
            json!([
                {"role": "user", "content": "hi"},
                {"role": "system", "content": [{"type": "text", "text": "More."}, {
                    "type": "tool_addition",
                    "tool": {"type": "tool_definition", "definition": {
                        "name": "store",
                        "description": "Tool store",
                        "eager_input_streaming": true,
                        "input_schema": {
                            "type": "object",
                            "properties": {"value": {"type": "string"}},
                            "required": ["value"]
                        }
                    }}
                }]},
                {"role": "assistant", "content": [{"type": "text", "text": "answer"}]}
            ])
        );
        assert!(betas(&assembly).contains(&"inline-tools-2026-09-15".to_string()));
    }

    #[test]
    fn strict_tools_full_schema_only_when_supported() {
        let strict_tool = Tool {
            name: "lookup".to_string(),
            description: "Tool lookup".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "value": {"type": "string"},
                    "optional": {"type": "number"}
                },
                "title": "StrictLookupInput"
            }),
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict: Strict::Prefer,
            })),
        };
        let ctx = tools_ctx(vec![strict_tool], vec![user("hi")]);

        // Supported: strict flag + strict-transformed schema (title preserved,
        // nullable wrapping against the ORIGINAL required list, full required).
        // Upstream Object.keys(properties) retains property insertion order.
        // The schema declares no required keys, so every property
        // gets the null-union wrap (upstream lines 100-113).
        let model = make_model(json!({"supportsStrictTools": true}));
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(assembly.body["tools"][0]["strict"], json!(true));
        assert_eq!(
            assembly.body["tools"][0]["input_schema"],
            json!({
                "type": "object",
                "title": "StrictLookupInput",
                "properties": {
                    "value": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                    "optional": {"anyOf": [{"type": "number"}, {"type": "null"}]}
                },
                "required": ["value", "optional"],
                "additionalProperties": false
            })
        );

        // Unsupported: plain legacy schema, no strict flag.
        let model = make_model(json!({}));
        let assembly = build(&model, &ctx, &opts());
        assert!(assembly.body["tools"][0].get("strict").is_none());
        assert_eq!(
            assembly.body["tools"][0]["input_schema"],
            json!({
                "type": "object",
                "properties": {
                    "value": {"type": "string"},
                    "optional": {"type": "number"}
                },
                "required": []
            })
        );

        // "require" without support is an error.
        let requiring = Tool {
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict: Strict::Require,
            })),
            ..tool_decl("lookup")
        };
        let ctx = tools_ctx(vec![requiring], vec![user("hi")]);
        let model = make_model(json!({}));
        assert!(build_request(&model, &cfg(), &ctx, &opts()).is_err());
    }

    #[test]
    fn oauth_tool_name_normalization_round_trip() {
        let tools = vec![
            tool_decl("todowrite"),
            tool_decl("find"),
            tool_decl("my_custom_tool"),
        ];
        let messages = vec![
            user("hi"),
            same_model_assistant(vec![tool_call("call_1", "todowrite")]),
        ];

        // OAuth: names matching CC tools (case-insensitive) get CC casing.
        let model = make_model(json!({}));
        let ctx = tools_ctx(tools.clone(), messages.clone());
        let options = AnthropicOptions {
            stream: StreamOptions {
                api_key: Some("sk-ant-oat01-test".to_string()),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        let names: Vec<&str> = assembly.body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["TodoWrite", "find", "my_custom_tool"]);
        assert_eq!(
            assembly.body["messages"][1]["content"][0]["name"],
            json!("TodoWrite")
        );

        // Non-OAuth: names pass through unchanged.
        let ctx = tools_ctx(tools, messages);
        let assembly = build(&model, &ctx, &opts());
        let names: Vec<&str> = assembly.body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["todowrite", "find", "my_custom_tool"]);
        assert_eq!(
            assembly.body["messages"][1]["content"][0]["name"],
            json!("todowrite")
        );
    }

    // ---- thinking (must-cover 4) ----

    #[test]
    fn thinking_disabled_param_gating() {
        let ctx = ctx_of(vec![user("Hello")]);

        // Budget-based reasoning model, thinking off: explicit disabled.
        let model = make_model(json!({}));
        let options = AnthropicOptions {
            thinking_enabled: Some(false),
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert_eq!(assembly.body["thinking"], json!({"type": "disabled"}));
        assert!(assembly.body.get("output_config").is_none());

        // thinkingLevelMap.off === null (Fable-style): param omitted.
        let mut model = make_model(json!({}));
        model.thinking_level_map = Some(BTreeMap::from([("off".to_string(), None)]));
        let assembly = build(&model, &ctx, &options);
        assert!(assembly.body.get("thinking").is_none());

        // Raw stream path with no thinking option: no thinking param at all.
        let model = make_model(json!({}));
        let assembly = build(&model, &ctx, &opts());
        assert!(assembly.body.get("thinking").is_none());
    }

    #[test]
    fn stream_simple_thinking_configs() {
        let ctx = ctx_of(vec![user("Hello")]);
        let simple =
            |model: &Model, reasoning: Option<ThinkingLevel>, budgets: Option<ThinkingBudgets>| {
                let options = SimpleStreamOptions {
                    reasoning,
                    thinking_budgets: budgets,
                    ..SimpleStreamOptions::default()
                };
                let anthropic_options = options_from_simple(model, &ctx, &options);
                build(model, &ctx, &anthropic_options)
            };

        // Legacy model, reasoning medium: budget-based thinking.
        let model = make_model(json!({}));
        let assembly = simple(&model, Some(ThinkingLevel::Medium), None);
        assert_eq!(
            assembly.body["thinking"],
            json!({"type": "enabled", "budget_tokens": 8192, "display": "summarized"})
        );
        assert!(assembly.body.get("output_config").is_none());
        assert_eq!(assembly.body["max_tokens"], json!(32000));

        // forceAdaptiveThinking: adaptive with effort, no budget.
        let model = make_model(json!({"forceAdaptiveThinking": true}));
        let assembly = simple(&model, Some(ThinkingLevel::Medium), None);
        assert_eq!(
            assembly.body["thinking"],
            json!({"type": "adaptive", "display": "summarized"})
        );
        assert_eq!(assembly.body["output_config"], json!({"effort": "medium"}));

        // Opt-out override on an adaptive-shaped model: back to budget.
        let model = make_model(json!({"forceAdaptiveThinking": false}));
        let assembly = simple(&model, Some(ThinkingLevel::Medium), None);
        assert_eq!(assembly.body["thinking"]["type"], json!("enabled"));
        assert!(assembly.body.get("output_config").is_none());

        // Fable-style off: null mapping omits the disabled param.
        let mut model = make_model(json!({"forceAdaptiveThinking": true}));
        model.thinking_level_map = Some(BTreeMap::from([("off".to_string(), None)]));
        let assembly = simple(&model, None, None);
        assert!(assembly.body.get("thinking").is_none());

        // xhigh maps through thinkingLevelMap (Opus 4.8-style).
        let mut model = make_model(json!({"forceAdaptiveThinking": true}));
        model.thinking_level_map = Some(BTreeMap::from([(
            "xhigh".to_string(),
            Some("xhigh".to_string()),
        )]));
        let assembly = simple(&model, Some(ThinkingLevel::Xhigh), None);
        assert_eq!(assembly.body["output_config"], json!({"effort": "xhigh"}));

        // Custom budgets and the minimal default.
        let model = make_model(json!({}));
        let assembly = simple(
            &model,
            Some(ThinkingLevel::Low),
            Some(ThinkingBudgets {
                low: Some(1024),
                ..ThinkingBudgets::default()
            }),
        );
        assert_eq!(assembly.body["thinking"]["budget_tokens"], json!(1024));
        let assembly = simple(&model, Some(ThinkingLevel::Minimal), None);
        assert_eq!(assembly.body["thinking"]["budget_tokens"], json!(1024));
    }

    /// The upstream mid-conversation-effort oracle runs with
    /// `cacheRetention: "none"`; mirror it so message shapes stay plain.
    fn no_cache() -> StreamOptions {
        StreamOptions {
            cache_retention: Some(CacheRetention::None),
            ..StreamOptions::default()
        }
    }

    #[test]
    fn mid_conversation_effort_markers() {
        // Capture 1: plain user turn with effort low.
        let model = managed_model();
        let ctx = ctx_of(vec![user("one")]);
        let options = AnthropicOptions {
            stream: no_cache(),
            thinking_enabled: Some(true),
            effort: Some(AnthropicEffort::Low),
            ..AnthropicOptions::default()
        };
        let first = build(&model, &ctx, &options);
        assert_eq!(
            first.body["messages"],
            json!([
                {"role": "user", "content": "one"},
                {"role": "system", "content": [], "output_config": {"effort": "low"}}
            ])
        );
        assert_eq!(first.body["output_config"], json!({"effort": "high"}));
        assert_eq!(
            first.body["thinking"],
            json!({
                "type": "adaptive",
                "display": "summarized",
                "block_binding": {"prefix_mismatch_behavior": "drop_block"}
            })
        );
        // The managed-effort compat also carries both beta features (upstream
        // getBetaFeatures, lines 1024-1028), in order.
        assert_eq!(
            betas(&first),
            [
                "mid-conversation-output-config-2026-07-01".to_string(),
                "thinking-binding-controls-2026-08-01".to_string(),
            ]
        );

        // Capture 2: historical marker prefix reconstructed, current marker appended.
        let ctx = ctx_of(vec![
            user("one"),
            assistant_msg(
                "anthropic",
                "anthropic-messages",
                "claude-fable-5-1",
                vec![
                    thinking("reasoning", Some("signature")),
                    AssistantBlock::Text(TextContent {
                        text: "answer".to_string(),
                        text_signature: None,
                    }),
                ],
                Some("low"),
            ),
            user("two"),
        ]);
        let options = AnthropicOptions {
            stream: no_cache(),
            thinking_enabled: Some(true),
            effort: Some(AnthropicEffort::High),
            ..AnthropicOptions::default()
        };
        let second = build(&model, &ctx, &options);
        assert_eq!(
            second.body["messages"],
            json!([
                {"role": "user", "content": "one"},
                {"role": "system", "content": [], "output_config": {"effort": "low"}},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "reasoning", "signature": "signature"},
                    {"type": "text", "text": "answer"}
                ]},
                {"role": "user", "content": "two"},
                {"role": "system", "content": [], "output_config": {"effort": "high"}}
            ])
        );

        // Legacy (no level) and other-provider assistants get no marker.
        let ctx = ctx_of(vec![
            user("one"),
            assistant_msg(
                "anthropic",
                "anthropic-messages",
                "claude-fable-5-1",
                vec![AssistantBlock::Text(TextContent {
                    text: "legacy".to_string(),
                    text_signature: None,
                })],
                None,
            ),
            assistant_msg(
                "other-provider",
                "anthropic-messages",
                "claude-fable-5-1",
                vec![AssistantBlock::Text(TextContent {
                    text: "foreign".to_string(),
                    text_signature: None,
                })],
                Some("low"),
            ),
        ]);
        let options = AnthropicOptions {
            stream: no_cache(),
            thinking_enabled: Some(true),
            effort: Some(AnthropicEffort::Medium),
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        let markers: Vec<&Value> = assembly.body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == json!("system"))
            .collect();
        assert_eq!(
            markers,
            vec![&json!({"role": "system", "content": [], "output_config": {"effort": "medium"}})]
        );

        // Non-managed model: top-level effort, no markers, no block_binding —
        // and neither mid-conversation beta.
        let model = make_model(json!({"forceAdaptiveThinking": true}));
        let ctx = ctx_of(vec![user("one")]);
        let options = AnthropicOptions {
            stream: no_cache(),
            thinking_enabled: Some(true),
            effort: Some(AnthropicEffort::Low),
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert_eq!(
            assembly.body["messages"],
            json!([{"role": "user", "content": "one"}])
        );
        assert_eq!(assembly.body["output_config"], json!({"effort": "low"}));
        assert_eq!(
            assembly.body["thinking"],
            json!({"type": "adaptive", "display": "summarized"})
        );
        assert_eq!(betas(&assembly), Vec::<String>::new());
    }

    /// Unsigned thinking replayed from another provider's assistant message
    /// converts to plain text (upstream lines 1332-1347: the signature check
    /// is provider-independent — a cross-model thinking block has no
    /// anthropic signature to replay).
    #[test]
    fn cross_model_unsigned_thinking_converts_to_text() {
        let model = make_model(json!({}));
        let ctx = ctx_of(vec![
            user("first"),
            assistant_msg(
                "openai",
                "openai-completions",
                "gpt-test",
                vec![
                    thinking("cross-model reasoning", None),
                    AssistantBlock::Text(TextContent {
                        text: "answer".to_string(),
                        text_signature: None,
                    }),
                ],
                None,
            ),
            user("second"),
        ]);
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(
            assembly.body["messages"][1]["content"],
            json!([
                {"type": "text", "text": "cross-model reasoning"},
                {"type": "text", "text": "answer"}
            ])
        );
    }

    // ---- temperature, max_tokens, metadata, fallbacks (must-cover 5) ----

    #[test]
    fn temperature_gating() {
        let ctx = ctx_of(vec![user("Hello")]);
        let with_temperature = |thinking_enabled: Option<bool>| AnthropicOptions {
            stream: StreamOptions {
                temperature: Some(0.0),
                ..StreamOptions::default()
            },
            thinking_enabled,
            ..AnthropicOptions::default()
        };

        // Unsupported (Opus 4.7-style): omitted.
        let model = make_model(json!({"supportsTemperature": false}));
        let assembly = build(&model, &ctx, &with_temperature(None));
        assert!(assembly.body.get("temperature").is_none());

        // Supported: sent, also when thinking is explicitly off.
        let model = make_model(json!({}));
        let assembly = build(&model, &ctx, &with_temperature(Some(false)));
        assert_eq!(assembly.body["temperature"], json!(0.0));

        // Extended thinking: omitted.
        let assembly = build(&model, &ctx, &with_temperature(Some(true)));
        assert!(assembly.body.get("temperature").is_none());

        // Managed-effort models: omitted.
        let model = managed_model();
        let assembly = build(&model, &ctx, &with_temperature(None));
        assert!(assembly.body.get("temperature").is_none());
    }

    #[test]
    fn max_tokens_and_metadata() {
        let ctx = ctx_of(vec![user("Hello")]);
        let model = make_model(json!({}));

        let assembly = build(&model, &ctx, &opts());
        assert_eq!(assembly.body["max_tokens"], json!(32000));

        let options = AnthropicOptions {
            stream: StreamOptions {
                max_tokens: Some(1024),
                metadata: Some(BTreeMap::from([("user_id".to_string(), json!("user-1"))])),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert_eq!(assembly.body["max_tokens"], json!(1024));
        assert_eq!(assembly.body["metadata"], json!({"user_id": "user-1"}));

        // Non-string user_id is ignored.
        let options = AnthropicOptions {
            stream: StreamOptions {
                metadata: Some(BTreeMap::from([("user_id".to_string(), json!(42))])),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert!(assembly.body.get("metadata").is_none());
    }

    #[test]
    fn fallbacks_only_when_non_empty() {
        let ctx = ctx_of(vec![user("Hello")]);
        let model = make_model(json!({
            "allowedFallbackModels": [
                {"provider": "anthropic", "model": "claude-opus-4-6", "cost": {"input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75}},
                {"provider": "anthropic", "model": "claude-sonnet-5", "cost": {"input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75}}
            ]
        }));
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(
            assembly.body["fallbacks"],
            json!([{"model": "claude-opus-4-6"}, {"model": "claude-sonnet-5"}])
        );
        assert!(betas(&assembly).contains(&"server-side-fallback-2026-07-01".to_string()));

        // Absent: no fallbacks field, no beta.
        let model = make_model(json!({}));
        let assembly = build(&model, &ctx, &opts());
        assert!(assembly.body.get("fallbacks").is_none());
        assert!(!betas(&assembly).contains(&"server-side-fallback-2026-07-01".to_string()));
    }

    // ---- headers (must-cover 6) ----

    #[test]
    fn auth_headers_and_missing_key_error() {
        let ctx = ctx_of(vec![user("Hello")]);
        let model = make_model(json!({}));

        // cfg key fallback: x-api-key injected, anthropic-version pinned.
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(header(&assembly, "x-api-key"), Some("test-key"));
        assert_eq!(header(&assembly, "anthropic-version"), Some("2023-06-01"));
        assert_eq!(header(&assembly, "User-Agent"), Some(ua().as_str()));

        // Explicit options key wins over cfg.
        let options = AnthropicOptions {
            stream: StreamOptions {
                api_key: Some("explicit-key".to_string()),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert_eq!(header(&assembly, "x-api-key"), Some("explicit-key"));

        // No credential at all: upstream assertRequestAuth throws.
        let error = build_request(&model, &cfg_with_key(""), &ctx, &opts()).unwrap_err();
        assert_eq!(error, "No API key for provider: anthropic");

        // Gateway-style Authorization header without a key: no x-api-key is
        // injected, no OAuth shaping.
        let prompt = prompt_ctx("Base.", vec![user("Hello")]);
        let options = AnthropicOptions {
            stream: StreamOptions {
                headers: Some(opt_headers(&[(
                    "Authorization",
                    Some("Bearer gateway-token"),
                )])),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build_request(&model, &cfg_with_key(""), &prompt, &options).unwrap();
        assert!(header(&assembly, "x-api-key").is_none());
        assert_eq!(
            header(&assembly, "Authorization"),
            Some("Bearer gateway-token")
        );
        assert!(!betas(&assembly).contains(&"oauth-2025-04-20".to_string()));
        assert_eq!(
            assembly.body["system"],
            json!([{"type": "text", "text": "Base.", "cache_control": {"type": "ephemeral"}}])
        );

        // Explicit User-Agent override.
        let options = AnthropicOptions {
            stream: StreamOptions {
                headers: Some(opt_headers(&[("User-Agent", Some("custom-client"))])),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert_eq!(header(&assembly, "User-Agent"), Some("custom-client"));
    }

    #[test]
    fn anthropic_beta_header_parsing_and_suppression() {
        let ctx = prompt_ctx("Base.", vec![user("Hello")]);
        let model = make_model(json!({}));

        // Explicit value: parsed into betas (split, trimmed) and re-emitted.
        let options = AnthropicOptions {
            stream: StreamOptions {
                headers: Some(opt_headers(&[("anthropic-beta", Some("custom-beta"))])),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert_eq!(assembly.body["betas"], json!(["custom-beta"]));
        assert_eq!(header(&assembly, "anthropic-beta"), Some("custom-beta"));

        // Explicit null: betas suppressed entirely.
        let options = AnthropicOptions {
            stream: StreamOptions {
                headers: Some(opt_headers(&[("anthropic-beta", None)])),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert!(assembly.body.get("betas").is_none());
        assert!(header(&assembly, "anthropic-beta").is_none());

        // Model headers participate; option headers override them.
        let mut model = make_model(json!({}));
        model.headers = Some(BTreeMap::from([(
            "anthropic-beta".to_string(),
            Some("model-beta-a, model-beta-b".to_string()),
        )]));
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(
            assembly.body["betas"],
            json!(["model-beta-a", "model-beta-b"])
        );
        let options = AnthropicOptions {
            stream: StreamOptions {
                headers: Some(opt_headers(&[("anthropic-beta", Some("option-beta"))])),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert_eq!(assembly.body["betas"], json!(["option-beta"]));
    }

    #[test]
    fn oauth_identity_headers_and_system() {
        let ctx = prompt_ctx("Base.", vec![user("Hello")]);
        let model = make_model(json!({}));
        let options = AnthropicOptions {
            stream: StreamOptions {
                api_key: Some("sk-ant-oat01-xyz".to_string()),
                ..StreamOptions::default()
            },
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);

        assert_eq!(
            header(&assembly, "Authorization"),
            Some("Bearer sk-ant-oat01-xyz")
        );
        assert!(header(&assembly, "x-api-key").is_none());
        assert_eq!(header(&assembly, "user-agent"), Some("claude-cli/2.1.280"));
        assert_eq!(header(&assembly, "x-app"), Some("cli"));
        let betas = betas(&assembly);
        assert!(betas.contains(&"claude-code-20250219".to_string()));
        assert!(betas.contains(&"oauth-2025-04-20".to_string()));
        assert_eq!(
            assembly.body["system"],
            json!([
                {"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude.", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "Base.", "cache_control": {"type": "ephemeral"}}
            ])
        );
    }

    #[test]
    fn session_affinity_headers() {
        let ctx = ctx_of(vec![user("Hello")]);
        let with_session = |compat: Value, retention: Option<CacheRetention>| {
            let model = make_model(compat);
            let options = AnthropicOptions {
                stream: StreamOptions {
                    session_id: Some("sess-1".to_string()),
                    cache_retention: retention,
                    ..StreamOptions::default()
                },
                ..AnthropicOptions::default()
            };
            build(&model, &ctx, &options)
        };

        // Default anthropic compat: no affinity headers.
        let assembly = with_session(json!({}), None);
        assert!(header(&assembly, "x-session-affinity").is_none());

        // Enabled: default header name.
        let assembly = with_session(json!({"sendSessionAffinityHeaders": true}), None);
        assert_eq!(header(&assembly, "x-session-affinity"), Some("sess-1"));

        // OpenRouter format: x-session-id.
        let assembly = with_session(
            json!({"sendSessionAffinityHeaders": true, "sessionAffinityFormat": "openrouter"}),
            None,
        );
        assert_eq!(header(&assembly, "x-session-id"), Some("sess-1"));
        assert!(header(&assembly, "x-session-affinity").is_none());

        // Retention none disables the affinity headers.
        let assembly = with_session(
            json!({"sendSessionAffinityHeaders": true}),
            Some(CacheRetention::None),
        );
        assert!(header(&assembly, "x-session-affinity").is_none());
    }

    #[test]
    fn copilot_dynamic_headers_and_bearer_auth() {
        let mut model = make_model(json!({}));
        model.provider = "github-copilot".to_string();
        let ctx = ctx_of(vec![user("Hello")]);
        let assembly = build(&model, &ctx, &opts());

        assert_eq!(header(&assembly, "Authorization"), Some("Bearer test-key"));
        assert!(header(&assembly, "x-api-key").is_none());
        assert_eq!(header(&assembly, "X-Initiator"), Some("user"));
        assert_eq!(
            header(&assembly, "Openai-Intent"),
            Some("conversation-edits")
        );
        assert!(header(&assembly, "Copilot-Vision-Request").is_none());

        // Last message not user: agent-initiated.
        let ctx = ctx_of(vec![
            user("Hello"),
            same_model_assistant(vec![AssistantBlock::Text(TextContent {
                text: "hi".to_string(),
                text_signature: None,
            })]),
        ]);
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(header(&assembly, "X-Initiator"), Some("agent"));

        // Images require the vision header.
        let mut model = make_model(json!({}));
        model.provider = "github-copilot".to_string();
        model.input = vec![ModelInput::Text, ModelInput::Image];
        let ctx = ctx_of(vec![user_blocks(vec![image_block()], TS)]);
        let assembly = build(&model, &ctx, &opts());
        assert_eq!(header(&assembly, "Copilot-Vision-Request"), Some("true"));
    }

    #[test]
    fn interleaved_thinking_beta_and_display() {
        let ctx = ctx_of(vec![user("Hello")]);
        let model = make_model(json!({}));
        let options = AnthropicOptions {
            thinking_enabled: Some(true),
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert!(betas(&assembly).contains(&"interleaved-thinking-2025-05-14".to_string()));
        assert_eq!(
            assembly.body["thinking"],
            json!({"type": "enabled", "budget_tokens": 1024, "display": "summarized"})
        );

        // Adaptive models have interleaved thinking built in: no beta.
        let model = make_model(json!({"forceAdaptiveThinking": true}));
        let assembly = build(&model, &ctx, &options);
        assert!(!betas(&assembly).contains(&"interleaved-thinking-2025-05-14".to_string()));

        // Explicit opt-out.
        let model = make_model(json!({}));
        let options = AnthropicOptions {
            thinking_enabled: Some(true),
            interleaved_thinking: Some(false),
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert!(!betas(&assembly).contains(&"interleaved-thinking-2025-05-14".to_string()));

        // Display override and explicit budget.
        let options = AnthropicOptions {
            thinking_enabled: Some(true),
            thinking_budget_tokens: Some(2048),
            thinking_display: Some(AnthropicThinkingDisplay::Omitted),
            ..AnthropicOptions::default()
        };
        let assembly = build(&model, &ctx, &options);
        assert_eq!(
            assembly.body["thinking"],
            json!({"type": "enabled", "budget_tokens": 2048, "display": "omitted"})
        );
    }

    // ---- helper unit tests ----

    #[test]
    fn claude_code_name_lookup_is_case_insensitive_and_exact() {
        assert_eq!(to_claude_code_name("todowrite"), "TodoWrite");
        assert_eq!(to_claude_code_name("BASH"), "Bash");
        assert_eq!(to_claude_code_name("find"), "find");
        assert_eq!(to_claude_code_name("my_custom_tool"), "my_custom_tool");
    }

    #[test]
    fn anthropic_tool_call_id_normalizer_matches_upstream() {
        assert_eq!(normalize_tool_call_id("a b|c#d"), "a_b_c_d");
        assert_eq!(normalize_tool_call_id("clean-id_1"), "clean-id_1");
        let long: String = "x".repeat(100);
        assert_eq!(normalize_tool_call_id(&long).len(), 64);
        // Astral characters count as two UTF-16 units, like the JS regex.
        assert_eq!(normalize_tool_call_id("\u{1F984}"), "__");
    }
}
