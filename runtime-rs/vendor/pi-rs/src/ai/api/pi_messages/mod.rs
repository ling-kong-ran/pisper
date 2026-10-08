//! pi-messages API (upstream `packages/ai/src/api/pi-messages.ts`): pi's own
//! message protocol streamed to a backend — a single POST of
//! `{ model, context, options }` to `<baseUrl>/messages` whose response is an
//! SSE stream of serialized assistant-message events plus a terminal
//! `done`/`error` event (the Radius gateway wire protocol; any backend
//! implementing it works, e.g. via a models.json custom provider with
//! `"api": "pi-messages"`).
//!
//! Coverage: the option extension (`PiMessagesOptions`, lines 33-38) with the
//! wire union (`PiMessagesEvent`, lines 44-87), the error-body parser and
//! response-error formatting with its `pi_messages_response_failure`
//! diagnostic (lines 89-156), the event converter accumulating the partial
//! message with streamed tool-argument JSON via `parseStreamingJson`
//! (lines 180-274), the custom SSE reader with CRLF normalization and the
//! `[DONE]` sentinel (lines 276-321), the catch-path `createErrorEvent`
//! (lines 323-345), `resolveCacheRetention` with the legacy
//! `PI_CACHE_RETENTION` env opt-in (lines 347-353), `stream` (lines 355-429)
//! and `streamSimple` (lines 431-443).
//!
//! Deviations from upstream, all structural:
//! - Upstream events carry the live `partial`; the port emits events without
//!   it and consumers reconstruct via `PartialAssistant` (the M2a contract).
//!   The terminal `done`/`error` messages are built from the converter's own
//!   accumulator, so the final message is upstream-faithful (id/tool name and
//!   parsed arguments included) even though the port's `start`/delta events
//!   carry fewer fields.
//! - Upstream `options.signal` is the port's non-serialized
//!   [`CancellationToken`](tokio_util::sync::CancellationToken): a
//!   pre-cancelled token or cancellation during the initial request fails
//!   before `Start` with the retry seam's `"Request aborted"` abort error; a
//!   cancellation after `Start` breaks the body read and the catch block
//!   (line 424) settles the `"aborted"` reason.
//! - `onPayload`/`onResponse` hooks (lines 387, 404) land with the callback
//!   plumbing (the same deferral as the other API ports; the port's
//!   `StreamOptions` carries no callbacks).
//! - Upstream issues one bare `fetch` with no retry; the port routes the
//!   initial request through the shared retry seam
//!   ([`crate::ai::retry::retry_provider_request`]) with `maxRetries`
//!   defaulting to `0`, so the default behavior is identical and opt-in
//!   retries cover transport failures and retryable statuses only.
//! - The URL base is `ProviderConfig.base_url` (the port's wiring) rather
//!   than `model.baseUrl`; both name the endpoint root in the port.
//! - The port applies `StreamOptions.timeout_ms` like the other API ports
//!   (the anthropic/openai-completions ports); upstream pi-messages uses a
//!   bare `fetch` and forwards no timeout.
//! - `streamSimple` gaps: upstream forwards a `debug` flag through its
//!   simple options, which the port drops (structurally forced — the port's
//!   `SimpleStreamOptions` carries no `debug` field; set it via
//!   [`PiMessagesOptions::debug`] on the direct path). A `done`/`error`
//!   wire event missing its `usage` object degrades to zeroed usage where
//!   upstream would assign the absent value through — a deliberate
//!   robustness divergence.
//! - An unknown wire event `type` is skipped. Upstream re-emits it as
//!   `{...event, partial}` — an event of a type outside the protocol — which
//!   the port's closed event enum cannot represent; no state mutation
//!   happens upstream either (no `switch` case matches).
//! - Upstream's JS sparse-array semantics (`partial.content[i] = ...` with
//!   gaps; dynamic property writes onto the wrong block kind) have no Rust
//!   equivalent: a content index beyond the next slot, or a delta/end for a
//!   missing or wrong-kind block, fails the stream (upstream would throw a
//!   `TypeError` for the missing-block cases and silently create stray
//!   properties otherwise).
//! - `statusText` in error messages comes from the `http` crate's canonical
//!   reason table, not the wire status line.
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order — same documented deviation as the other request builders.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::ai::api::openai_completions::stream::parse_streaming_json;
use crate::ai::api::{http_client, request_signal, ApiImpl, REQUEST_WAS_ABORTED};
use crate::ai::retry::{retry_provider_request, ProviderError};
use crate::ai::transcript::TranscriptContext;
use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, AssistantMessageDiagnostic, DiagnosticCode,
    DiagnosticErrorInfo,
};
use crate::ai::types::options::{ProviderEnv, SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{CacheRetention, StopReason, ThinkingLevel, ToolChoice, Usage};
use crate::ai::types::Model;
use crate::ai::{now_ms, ProviderConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The API id stamped on every emitted message.
const API: &str = "pi-messages";

/// The pi-messages API implementation (upstream module `stream`/`streamSimple`).
pub struct PiMessages;

/// The wire `type` literals of the pi-messages event protocol
/// (pi-messages.ts:54-87). Unknown types are skipped (module docs).
const PI_MESSAGES_EVENT_TYPES: [&str; 12] = [
    "start",
    "text_start",
    "text_delta",
    "text_end",
    "thinking_start",
    "thinking_delta",
    "thinking_end",
    "toolcall_start",
    "toolcall_delta",
    "toolcall_end",
    "done",
    "error",
];

/// Upstream `PiMessagesOptions.toolChoice` (pi-messages.ts:35): the string
/// choices plus the forced-function object form. The string form lives in
/// [`PiMessagesNamedToolChoice`] because an untagged unit variant has no wire
/// representation of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PiMessagesToolChoice {
    Named(PiMessagesNamedToolChoice),
    Function(PiMessagesFunctionChoice),
}

/// The string tool-choice values (`"auto" | "none" | "required"`,
/// pi-messages.ts:35).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiMessagesNamedToolChoice {
    Auto,
    None,
    Required,
}

/// Upstream `{ type: "function"; function: { name: string } }`
/// (pi-messages.ts:35).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PiMessagesFunctionChoice {
    /// Always the literal `"function"`.
    #[serde(rename = "type")]
    pub function_type: PiMessagesFunctionType,
    pub function: PiMessagesFunctionName,
}

/// The literal `"function"` tag of [`PiMessagesFunctionChoice::function_type`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PiMessagesFunctionType {
    #[serde(rename = "function")]
    Function,
}

/// Upstream `{ name: string }` (pi-messages.ts:35).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PiMessagesFunctionName {
    pub name: String,
}

impl From<ToolChoice> for PiMessagesToolChoice {
    fn from(choice: ToolChoice) -> Self {
        match choice {
            ToolChoice::Auto => PiMessagesToolChoice::Named(PiMessagesNamedToolChoice::Auto),
            ToolChoice::None => PiMessagesToolChoice::Named(PiMessagesNamedToolChoice::None),
        }
    }
}

/// Upstream `PiMessagesOptions` (pi-messages.ts:33-38) minus the
/// signal/fetch/callback fields that land with the callback plumbing: the
/// base [`StreamOptions`] flattened with the API-specific extensions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PiMessagesOptions {
    /// Base options inherited from upstream `StreamOptions`.
    pub stream: StreamOptions,
    /// Reasoning/thinking level forwarded to the backend
    /// (pi-messages.ts:34). `streamSimple` maps its neutral `reasoning`
    /// field here.
    pub reasoning: Option<ThinkingLevel>,
    /// Tool selection forwarded to the backend (pi-messages.ts:35).
    /// `streamSimple` maps its neutral `toolChoice` here.
    pub tool_choice: Option<PiMessagesToolChoice>,
    /// Ask the backend for debug metadata, e.g. routing response headers
    /// (pi-messages.ts:37): appends `debug=1` to the request URL.
    pub debug: bool,
}

impl PiMessagesOptions {
    /// Base options only, for direct `stream`-path callers that pass no
    /// API-specific extensions (upstream: the extension fields `undefined`).
    pub fn from_stream(stream: StreamOptions) -> Self {
        PiMessagesOptions {
            stream,
            reasoning: None,
            tool_choice: None,
            debug: false,
        }
    }
}

/// Upstream `streamSimple` (pi-messages.ts:431-443): the neutral simple
/// options map onto the API-specific extension fields, then everything
/// delegates to the same request path.
pub fn options_from_simple(options: &SimpleStreamOptions) -> PiMessagesOptions {
    PiMessagesOptions {
        stream: options.stream.clone(),
        reasoning: options.reasoning,
        tool_choice: options.tool_choice.map(PiMessagesToolChoice::from),
        debug: false,
    }
}

/// Upstream `resolveCacheRetention` (pi-messages.ts:347-353): an explicit
/// option wins; otherwise only the legacy `PI_CACHE_RETENTION=long` env
/// opt-in maps to `long`, and anything else stays unset (the backend
/// defaults apply).
fn resolve_cache_retention(
    retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> Option<CacheRetention> {
    if let Some(retention) = retention {
        return Some(retention);
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
    (from_env.as_deref() == Some("long")).then_some(CacheRetention::Long)
}

/// The request body (upstream lines 375-386): `{ model, context, options }`
/// with `undefined` option fields dropped (upstream relies on
/// `JSON.stringify` dropping them).
pub fn build_payload(model: &Model, ctx: &TranscriptContext, options: &PiMessagesOptions) -> Value {
    let mut request_options = Map::new();
    if let Some(temperature) = options.stream.temperature {
        request_options.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_tokens) = options.stream.max_tokens {
        request_options.insert("maxTokens".to_string(), json!(max_tokens));
    }
    if let Some(reasoning) = options.reasoning {
        request_options.insert("reasoning".to_string(), json!(reasoning));
    }
    if let Some(cache_retention) =
        resolve_cache_retention(options.stream.cache_retention, options.stream.env.as_ref())
    {
        request_options.insert("cacheRetention".to_string(), json!(cache_retention));
    }
    if let Some(session_id) = &options.stream.session_id {
        request_options.insert("sessionId".to_string(), json!(session_id));
    }
    if let Some(tool_choice) = &options.tool_choice {
        // Serialization cannot fail (plain JSON data).
        let wire = serde_json::to_value(tool_choice).expect("tool choice serializes");
        request_options.insert("toolChoice".to_string(), wire);
    }
    let messages = serde_json::to_value(ctx.messages()).expect("transcript messages serialize");
    json!({
        "model": model.id,
        "context": { "messages": messages },
        "options": request_options,
    })
}

// ---- error body parsing and formatting (pi-messages.ts:89-156) ----

/// A thrown error inside the stream task: `message` becomes the error
/// event's `errorMessage`; response failures additionally carry the
/// `pi_messages_response_failure` diagnostic (upstream
/// `PiMessagesResponseError`).
#[derive(Debug, Clone)]
struct StreamFailure {
    message: String,
    // Diagnostics are only present on response failures; keep the common error small.
    response: Option<Box<ResponseFailure>>,
}

impl StreamFailure {
    fn plain(message: impl Into<String>) -> Self {
        StreamFailure {
            message: message.into(),
            response: None,
        }
    }
}

/// The response-error payload stripped out of [`StreamFailure`] so the
/// diagnostic can be rebuilt after the retry seam hands back the final
/// error.
#[derive(Debug, Clone)]
struct ResponseFailure {
    message: String,
    code: Option<String>,
    diagnostic_details: Value,
}

/// Upstream `parsePiMessagesErrorBody` (pi-messages.ts:110-118): the body
/// parses as JSON and carries a non-array `error` object; otherwise `None`.
fn parse_error_body(body: &str) -> Option<Value> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    if parsed.get("error").is_some_and(Value::is_object) {
        Some(parsed)
    } else {
        None
    }
}

/// Upstream `truncateDiagnosticString` (pi-messages.ts:120-123).
fn truncate_diagnostic_string(value: &str) -> String {
    const MAX_LENGTH: usize = 8192;
    if value.chars().count() > MAX_LENGTH {
        let truncated: String = value.chars().take(MAX_LENGTH).collect();
        format!("{truncated}\u{2026}")
    } else {
        value.to_string()
    }
}

/// Upstream `formatPiMessagesResponseError` + `createPiMessagesResponseError`
/// (pi-messages.ts:125-156): the `` "{status} {statusText}: {message|body}
/// ({code})" `` message and the `pi_messages_response_failure` diagnostic
/// details object.
fn create_response_failure(
    model: &Model,
    url: &str,
    status: u16,
    status_text: &str,
    body: &str,
) -> ResponseFailure {
    let error_body = parse_error_body(body);
    let message = error_body
        .as_ref()
        .and_then(|parsed| parsed.pointer("/error/message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let code = error_body
        .as_ref()
        .and_then(|parsed| parsed.pointer("/error/code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let suffix = message.as_deref().unwrap_or(body);
    let code_suffix = code
        .as_deref()
        .map(|code| format!(" ({code})"))
        .unwrap_or_default();
    let message = format!("{status} {status_text}: {suffix}{code_suffix}");

    let mut details = Map::new();
    details.insert("version".to_string(), json!(1));
    details.insert("provider".to_string(), json!(model.provider));
    details.insert("model".to_string(), json!(model.id));
    details.insert("url".to_string(), json!(url));
    details.insert("status".to_string(), json!(status));
    details.insert("statusText".to_string(), json!(status_text));
    match &error_body {
        Some(parsed) => {
            details.insert("error".to_string(), parsed["error"].clone());
        }
        None => {
            details.insert("body".to_string(), json!(truncate_diagnostic_string(body)));
        }
    }
    details.insert("timestampMs".to_string(), json!(now_ms()));
    ResponseFailure {
        message,
        code,
        diagnostic_details: Value::Object(details),
    }
}

// ---- auth (pi-messages.ts:365-368) ----

/// The request credential: the options key (upstream `options?.apiKey`),
/// then the provider credential `cfg.api_key` (the port's wiring). Upstream
/// throws when both are missing.
fn resolve_api_key(
    model: &Model,
    cfg: &ProviderConfig,
    options: &PiMessagesOptions,
) -> Result<String, StreamFailure> {
    if let Some(key) = options
        .stream
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
    {
        return Ok(key.to_string());
    }
    if !cfg.api_key.is_empty() {
        return Ok(cfg.api_key.clone());
    }
    Err(StreamFailure::plain(format!(
        "No API key provided for provider \"{}\"",
        model.provider
    )))
}

// ---- request send (pi-messages.ts:370-412) with the retry seam ----

/// Send the assembled request (upstream lines 370-412). Upstream issues one
/// bare `fetch`; the port wraps the initial request in the provider retry
/// seam with `options.maxRetries` defaulting to `0` (module docs). Only
/// request setup retries — once stream bytes flow, errors are terminal.
async fn send_stream_request(
    cfg: &ProviderConfig,
    model: &Model,
    url: &str,
    payload: &Value,
    options: &PiMessagesOptions,
    signal: &CancellationToken,
) -> Result<reqwest::Response, StreamFailure> {
    let api_key = resolve_api_key(model, cfg, options)?;
    let mut headers = reqwest::header::HeaderMap::new();
    let mut insert_header = |name: &str, value: String| -> Result<(), StreamFailure> {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            StreamFailure::plain(format!("Invalid header name \"{name}\": {error}"))
        })?;
        let value = reqwest::header::HeaderValue::from_str(&value).map_err(|error| {
            StreamFailure::plain(format!("Invalid header value for \"{name}\": {error}"))
        })?;
        headers.insert(name, value);
        Ok(())
    };
    insert_header("authorization", format!("Bearer {api_key}"))?;
    insert_header("accept", "text/event-stream".to_string())?;
    insert_header("content-type", "application/json".to_string())?;
    // Upstream spread `{...providerHeadersToRecord(options?.headers)}`:
    // non-null entries override the defaults, null entries are dropped.
    for (name, value) in options.stream.headers.iter().flatten() {
        if let Some(value) = value {
            insert_header(name, value.clone())?;
        }
    }

    let mut request = http_client().post(url).headers(headers).json(payload);
    if let Some(ms) = options.stream.timeout_ms {
        request = request.timeout(std::time::Duration::from_millis(ms));
    }

    // The response-error diagnostic details are produced inside the retry
    // closure; the last failure's payload is stashed for the final error.
    let last_failure: Arc<Mutex<Option<ResponseFailure>>> = Arc::new(Mutex::new(None));
    let max_retries = options.stream.max_retries.unwrap_or(0);
    let max_retry_delay_ms = options.stream.max_retry_delay_ms;
    let result = retry_provider_request(max_retries, max_retry_delay_ms, Some(signal), || async {
        let response = request
            .try_clone()
            .expect("JSON request body is buffered and clonable")
            .send()
            .await
            .map_err(|error| ProviderError::transport(error.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let status_code = status.as_u16();
        let response_headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        let status_text = status.canonical_reason().unwrap_or("");
        let failure = create_response_failure(model, url, status_code, status_text, &body);
        let message = failure.message.clone();
        *last_failure.lock().unwrap() = Some(failure);
        Err(ProviderError::http(status_code, response_headers, message))
    })
    .await;
    match result {
        Ok(response) => Ok(response),
        Err(error) => Err(StreamFailure {
            message: error.message,
            response: last_failure.lock().unwrap().take().map(Box::new),
        }),
    }
}

// ---- wire events (pi-messages.ts:44-87) ----

/// Upstream `PiMessagesEvent`: the serialized assistant-message event sent by
/// a pi-messages backend, discriminated by the `type` field with camelCase
/// members. `done`/`error` usage is optional on the wire here so a missing
/// usage object degrades to zeroed usage instead of failing the event parse.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum PiMessagesEvent {
    Start,
    TextStart {
        content_index: usize,
    },
    TextDelta {
        content_index: usize,
        delta: String,
    },
    TextEnd {
        content_index: usize,
        content: String,
        content_signature: Option<String>,
    },
    ThinkingStart {
        content_index: usize,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
    },
    ThinkingEnd {
        content_index: usize,
        content: String,
        content_signature: Option<String>,
        redacted: Option<bool>,
    },
    ToolcallStart {
        content_index: usize,
        id: String,
        tool_name: String,
    },
    ToolcallDelta {
        content_index: usize,
        delta: String,
    },
    ToolcallEnd {
        content_index: usize,
        tool_call: ToolCall,
    },
    Done {
        reason: SuccessReason,
        usage: Option<Usage>,
        response_id: Option<String>,
        provider_thinking_level: Option<String>,
        rewrite: Option<Value>,
    },
    Error {
        reason: ErrorReason,
        usage: Option<Usage>,
        error_message: Option<String>,
        response_id: Option<String>,
        provider_thinking_level: Option<String>,
        rewrite: Option<Value>,
    },
}

// ---- event converter (pi-messages.ts:180-274) ----

/// Accumulation state for one stream: the upstream converter closure's
/// `partial` message and `toolJson` scratch per tool-call content index.
struct EventConverter {
    partial: AssistantMessage,
    tool_json: HashMap<usize, String>,
}

impl EventConverter {
    fn new(model: &Model) -> Self {
        EventConverter {
            partial: AssistantMessage {
                content: Vec::new(),
                api: API.to_string(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                provider_thinking_level: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: now_ms(),
            },
            tool_json: HashMap::new(),
        }
    }

    /// Upstream `createEventConverter`'s returned function: mutate the
    /// partial, then map the wire event onto the canonical event protocol.
    /// The port's events carry no `partial` field (module docs); the final
    /// `done`/`error` message is built from the accumulator.
    fn convert(&mut self, event: PiMessagesEvent) -> Result<AssistantMessageEvent, String> {
        match event {
            PiMessagesEvent::Start => Ok(AssistantMessageEvent::Start {
                message: self.partial.clone(),
            }),
            PiMessagesEvent::TextStart { content_index } => {
                assign_block(
                    &mut self.partial.content,
                    content_index,
                    AssistantBlock::Text(TextContent {
                        text: String::new(),
                        text_signature: None,
                    }),
                )?;
                Ok(AssistantMessageEvent::TextStart { content_index })
            }
            PiMessagesEvent::TextDelta {
                content_index,
                delta,
            } => {
                match self.partial.content.get_mut(content_index) {
                    Some(AssistantBlock::Text(text)) => text.text.push_str(&delta),
                    _ => {
                        return Err(format!(
                        "pi-messages text_delta event has no text block at index {content_index}"
                    ))
                    }
                }
                Ok(AssistantMessageEvent::TextDelta {
                    content_index,
                    delta,
                })
            }
            PiMessagesEvent::TextEnd {
                content_index,
                content,
                content_signature,
            } => {
                match self.partial.content.get_mut(content_index) {
                    Some(block @ AssistantBlock::Text(_)) => {
                        *block = AssistantBlock::Text(TextContent {
                            text: content.clone(),
                            text_signature: content_signature,
                        });
                    }
                    _ => {
                        return Err(format!(
                            "pi-messages text_end event has no text block at index {content_index}"
                        ))
                    }
                }
                Ok(AssistantMessageEvent::TextEnd {
                    content_index,
                    content,
                })
            }
            PiMessagesEvent::ThinkingStart { content_index } => {
                assign_block(
                    &mut self.partial.content,
                    content_index,
                    AssistantBlock::Thinking(ThinkingContent {
                        thinking: String::new(),
                        thinking_signature: None,
                        redacted: None,
                    }),
                )?;
                Ok(AssistantMessageEvent::ThinkingStart { content_index })
            }
            PiMessagesEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                match self.partial.content.get_mut(content_index) {
                    Some(AssistantBlock::Thinking(thinking)) => {
                        thinking.thinking.push_str(&delta)
                    }
                    _ => {
                        return Err(format!(
                            "pi-messages thinking_delta event has no thinking block at index {content_index}"
                        ))
                    }
                }
                Ok(AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta,
                })
            }
            PiMessagesEvent::ThinkingEnd {
                content_index,
                content,
                content_signature,
                redacted,
            } => {
                match self.partial.content.get_mut(content_index) {
                    Some(block @ AssistantBlock::Thinking(_)) => {
                        *block = AssistantBlock::Thinking(ThinkingContent {
                            thinking: content.clone(),
                            thinking_signature: content_signature,
                            redacted,
                        });
                    }
                    _ => {
                        return Err(format!(
                            "pi-messages thinking_end event has no thinking block at index {content_index}"
                        ))
                    }
                }
                Ok(AssistantMessageEvent::ThinkingEnd {
                    content_index,
                    content,
                })
            }
            PiMessagesEvent::ToolcallStart {
                content_index,
                id,
                tool_name,
            } => {
                assign_block(
                    &mut self.partial.content,
                    content_index,
                    AssistantBlock::ToolCall(ToolCall {
                        id,
                        name: tool_name,
                        arguments: json!({}),
                        thought_signature: None,
                        namespace: None,
                    }),
                )?;
                self.tool_json.insert(content_index, String::new());
                Ok(AssistantMessageEvent::ToolcallStart { content_index })
            }
            PiMessagesEvent::ToolcallDelta {
                content_index,
                delta,
            } => {
                // Upstream: `json = (toolJson.get(idx) ?? "") + delta`.
                let accumulated = {
                    let json = self.tool_json.entry(content_index).or_default();
                    json.push_str(&delta);
                    json.clone()
                };
                let parsed = parse_streaming_json(&accumulated);
                match self.partial.content.get_mut(content_index) {
                    Some(AssistantBlock::ToolCall(call)) => call.arguments = parsed,
                    _ => {
                        return Err(format!(
                            "pi-messages toolcall_delta event has no tool call block at index {content_index}"
                        ))
                    }
                }
                Ok(AssistantMessageEvent::ToolcallDelta {
                    content_index,
                    delta,
                })
            }
            PiMessagesEvent::ToolcallEnd {
                content_index,
                tool_call,
            } => {
                // Upstream `Object.assign(partial.content[idx], toolCall)` —
                // the complete tool call replaces the placeholder wholesale.
                match self.partial.content.get_mut(content_index) {
                    Some(block @ AssistantBlock::ToolCall(_)) => {
                        *block = AssistantBlock::ToolCall(tool_call.clone());
                    }
                    _ => {
                        return Err(format!(
                            "pi-messages toolcall_end event has no tool call block at index {content_index}"
                        ))
                    }
                }
                self.tool_json.remove(&content_index);
                Ok(AssistantMessageEvent::ToolcallEnd {
                    content_index,
                    tool_call,
                })
            }
            PiMessagesEvent::Done {
                reason,
                usage,
                response_id,
                provider_thinking_level,
                rewrite,
            } => {
                self.partial.stop_reason = StopReason::from(reason);
                self.partial.usage = usage.unwrap_or_default();
                self.partial.response_id = response_id;
                if let Some(level) = provider_thinking_level {
                    self.partial.provider_thinking_level = Some(level);
                }
                append_rewrite_diagnostic(&mut self.partial, rewrite);
                Ok(AssistantMessageEvent::Done {
                    reason,
                    message: self.partial.clone(),
                })
            }
            PiMessagesEvent::Error {
                reason,
                usage,
                error_message,
                response_id,
                provider_thinking_level,
                rewrite,
            } => {
                self.partial.stop_reason = StopReason::from(reason);
                self.partial.usage = usage.unwrap_or_default();
                self.partial.error_message = error_message;
                self.partial.response_id = response_id;
                if let Some(level) = provider_thinking_level {
                    self.partial.provider_thinking_level = Some(level);
                }
                append_rewrite_diagnostic(&mut self.partial, rewrite);
                Ok(AssistantMessageEvent::Error {
                    reason,
                    error: self.partial.clone(),
                })
            }
        }
    }
}

/// Wire content-block slot assignment (upstream
/// `partial.content[event.contentIndex] = ...`): in-place overwrite or the
/// next slot; a gap would be a JS sparse array, which the port cannot
/// represent (module docs).
fn assign_block(
    content: &mut Vec<AssistantBlock>,
    index: usize,
    block: AssistantBlock,
) -> Result<(), String> {
    if index == content.len() {
        content.push(block);
    } else if index < content.len() {
        content[index] = block;
    } else {
        return Err(format!(
            "pi-messages event content index {index} would leave a gap"
        ));
    }
    Ok(())
}

/// Upstream `appendRewriteDiagnostic` (pi-messages.ts:169-178): a
/// `pi_messages_rewrite` diagnostic summarizing a server-side message
/// rewrite.
fn append_rewrite_diagnostic(message: &mut AssistantMessage, rewrite: Option<Value>) {
    let Some(rewrite) = rewrite else {
        return;
    };
    message
        .diagnostics
        .get_or_insert_with(Vec::new)
        .push(AssistantMessageDiagnostic {
            r#type: "pi_messages_rewrite".to_string(),
            timestamp: now_ms(),
            error: None,
            details: Some(rewrite),
        });
}

// ---- catch-path error event (pi-messages.ts:323-345) ----

/// Upstream `createErrorEvent`: a fresh assistant message (the accumulated
/// partial is discarded, matching upstream) carrying the thrown error's
/// message, plus the `pi_messages_response_failure` diagnostic for response
/// errors. A cancelled request signal selects the `aborted` reason
/// (`options?.signal?.aborted ?? false`).
fn create_error_event(
    model: &Model,
    failure: &StreamFailure,
    aborted: bool,
) -> AssistantMessageEvent {
    let mut message = AssistantMessage {
        content: Vec::new(),
        api: API.to_string(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: if aborted {
            StopReason::Aborted
        } else {
            StopReason::Error
        },
        deferred: None,
        error_message: Some(failure.message.clone()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    };
    if let Some(response) = &failure.response {
        message.diagnostics = Some(vec![AssistantMessageDiagnostic {
            r#type: "pi_messages_response_failure".to_string(),
            timestamp: now_ms(),
            error: Some(DiagnosticErrorInfo {
                name: Some("PiMessagesResponseError".to_string()),
                message: failure.message.clone(),
                stack: None,
                code: response.code.clone().map(DiagnosticCode::String),
            }),
            details: Some(response.diagnostic_details.clone()),
        }]);
    }
    AssistantMessageEvent::Error {
        reason: if aborted {
            ErrorReason::Aborted
        } else {
            ErrorReason::Error
        },
        error: message,
    }
}

// ---- SSE reading (pi-messages.ts:276-321) ----

/// Upstream line 285: `buffer = buffer.replace(/\r\n/g, "\n")` on the whole
/// accumulated buffer each round. Byte-level because multibyte UTF-8 never
/// contains CR/LF bytes; a trailing CR waits for its LF like upstream.
fn normalize_crlf(buffer: &mut Vec<u8>) {
    let mut normalized = Vec::with_capacity(buffer.len());
    let mut iter = buffer.iter().copied().peekable();
    while let Some(byte) = iter.next() {
        if byte == b'\r' && iter.peek() == Some(&b'\n') {
            continue;
        }
        normalized.push(byte);
    }
    *buffer = normalized;
}

/// Upstream `readPiMessagesEvents`'s inner loop: one `data:` line per
/// `\n\n`-delimited block, trimmed, `[DONE]` and empty blocks skipped,
/// unknown `type`s skipped (module docs), malformed JSON failing the stream
/// like upstream's `JSON.parse`.
fn parse_frame(frame: &[u8]) -> Result<Option<PiMessagesEvent>, StreamFailure> {
    let text = String::from_utf8_lossy(frame);
    let Some(data) = text
        .split('\n')
        .find_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty() && *data != "[DONE]")
    else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(data).map_err(|error| {
        StreamFailure::plain(format!("Could not parse pi-messages event: {error}"))
    })?;
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !PI_MESSAGES_EVENT_TYPES.contains(&event_type.as_str()) {
        return Ok(None);
    }
    serde_json::from_value(value).map(Some).map_err(|error| {
        StreamFailure::plain(format!(
            "Could not parse pi-messages {event_type} event: {error}"
        ))
    })
}

/// Consume one parsed wire event: convert, emit, and report whether it was
/// terminal (upstream lines 414-420).
async fn emit_event(
    converter: &mut EventConverter,
    event: PiMessagesEvent,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<bool, StreamFailure> {
    let converted = converter.convert(event).map_err(StreamFailure::plain)?;
    let terminal = matches!(
        converted,
        AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
    );
    let _ = tx.send(converted).await;
    Ok(terminal)
}

async fn drive_stream(
    cfg: &ProviderConfig,
    model: &Model,
    ctx: &TranscriptContext,
    options: &PiMessagesOptions,
    signal: &CancellationToken,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), StreamFailure> {
    let mut url = format!("{}/messages", cfg.base_url.trim_end_matches('/'));
    if options.debug {
        url = format!("{url}?debug=1");
    }
    let payload = build_payload(model, ctx, options);
    let response = send_stream_request(cfg, model, &url, &payload, options, signal).await?;

    let mut converter = EventConverter::new(model);
    let mut buffer: Vec<u8> = Vec::new();
    let mut events = response.bytes_stream();
    // Upstream reads the body with the request signal attached; the select
    // breaks the read the moment the token cancels.
    loop {
        let chunk = tokio::select! {
            biased;
            _ = signal.cancelled() => {
                return Err(StreamFailure::plain(REQUEST_WAS_ABORTED.to_string()));
            }
            next = events.next() => match next {
                Some(chunk) => chunk,
                None => break,
            },
        };
        let chunk = chunk.map_err(|error| StreamFailure::plain(error.to_string()))?;
        buffer.extend_from_slice(&chunk);
        normalize_crlf(&mut buffer);
        while let Some(split) = buffer.windows(2).position(|window| window == b"\n\n") {
            let frame: Vec<u8> = buffer.drain(..split + 2).collect();
            if let Some(event) = parse_frame(&frame)? {
                if emit_event(&mut converter, event, tx).await? {
                    return Ok(());
                }
            }
        }
    }
    if signal.is_cancelled() {
        return Err(StreamFailure::plain(REQUEST_WAS_ABORTED.to_string()));
    }

    // Upstream lines 302-307: a non-whitespace trailing buffer without its
    // closing blank line still parses.
    let trailing = String::from_utf8_lossy(&buffer);
    if !trailing.trim().is_empty() {
        if let Some(event) = parse_frame(&buffer)? {
            if emit_event(&mut converter, event, tx).await? {
                return Ok(());
            }
        }
    }

    // Upstream line 422.
    Err(StreamFailure::plain(format!(
        "{} stream ended without a terminal event",
        model.provider
    )))
}

async fn run_stream_task(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: PiMessagesOptions,
    tx: mpsc::Sender<AssistantMessageEvent>,
) {
    let signal = request_signal(&options.stream.signal);
    match drive_stream(&cfg, &model, &ctx, &options, &signal, &tx).await {
        Ok(()) => {}
        Err(failure) => {
            let _ = tx
                .send(create_error_event(&model, &failure, signal.is_cancelled()))
                .await;
        }
    }
}

fn run_stream(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: PiMessagesOptions,
) -> mpsc::Receiver<AssistantMessageEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        run_stream_task(cfg, model, ctx, options, tx).await;
    });
    rx
}

impl PiMessages {
    /// Upstream `stream` (pi-messages.ts:355-429): the entry point taking the
    /// API-specific options. The [`ApiImpl`] methods delegate here.
    pub fn stream_with_options(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &PiMessagesOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        run_stream(cfg.clone(), model.clone(), ctx.clone(), options.clone())
    }
}

impl ApiImpl for PiMessages {
    fn stream(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream `stream` takes the API-specific option extension on top of
        // the base options; the port's `StreamOptions` is the base set, so the
        // extension fields stay unset for direct `stream` calls.
        let options = PiMessagesOptions::from_stream(options.clone());
        self.stream_with_options(cfg, model, ctx, &options)
    }

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream `streamSimple` (pi-messages.ts:431-443) maps the neutral
        // options onto the API-specific fields and delegates to `stream`.
        let options = options_from_simple(options);
        self.stream_with_options(cfg, model, ctx, &options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::REQUEST_ABORTED;
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
    use crate::ai::types::events::{ErrorReason, PartialAssistant, SuccessReason};
    use crate::ai::types::message::{
        AssistantBlock, AssistantMessage, Message, StringOrBlocks, UserMessage,
    };
    use crate::ai::types::primitives::{StopReason, ToolChoice, Usage, UsageCost};
    use crate::ai::types::ModelInput;

    const TS: i64 = 1758240000000;

    // ---- fixtures (pi-messages.test.ts:69-95) ----

    fn make_model(base_url: &str) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "auto".to_string(),
            name: "Radius Auto".to_string(),
            api: "pi-messages".to_string(),
            provider: "radius".to_string(),
            base_url: base_url.to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: crate::ai::types::primitives::ModelCost::default(),
            context_window: 128000,
            max_tokens: 16384,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    fn keyed_cfg(server: &wiremock::MockServer) -> ProviderConfig {
        ProviderConfig {
            base_url: format!("{}/v1", server.uri()),
            // The options key must win over the ambient provider key.
            api_key: "cfg-key".to_string(),
            max_tokens: 16384,
        }
    }

    fn empty_cfg(server: &wiremock::MockServer) -> ProviderConfig {
        ProviderConfig {
            base_url: format!("{}/v1", server.uri()),
            api_key: String::new(),
            max_tokens: 16384,
        }
    }

    fn user_ctx() -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("Hello".to_string()),
                timestamp: TS,
            })],
            tools: None,
        })
    }

    /// pi-messages.test.ts:88-95.
    fn usage_fixture() -> Value {
        json!({
            "input": 10,
            "output": 5,
            "cacheRead": 0,
            "cacheWrite": 0,
            "totalTokens": 15,
            "cost": {"input": 0.1, "output": 0.2, "cacheRead": 0, "cacheWrite": 0, "total": 0.3}
        })
    }

    fn expected_usage() -> Usage {
        Usage {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 15,
            cost: UsageCost {
                input: 0.1,
                output: 0.2,
                cache_read: 0.0,
                cache_write: 0.0,
                total: 0.3,
            },
        }
    }

    // ---- SSE helpers ----

    fn sse(events: &[Value]) -> wiremock::ResponseTemplate {
        let mut body = String::new();
        for event in events {
            body.push_str(&format!("data: {event}\n\n"));
        }
        wiremock::ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body)
    }

    async fn mount_sse(server: &wiremock::MockServer, body: wiremock::ResponseTemplate) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(body)
            .mount(server)
            .await;
    }

    async fn collect_stream(
        server: &wiremock::MockServer,
        options: &StreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = PiMessages;
        let cfg = keyed_cfg(server);
        let model = make_model(&cfg.base_url);
        let mut rx = api.stream(&cfg, &model, &user_ctx(), options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    async fn collect_simple(
        cfg: &ProviderConfig,
        options: &SimpleStreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = PiMessages;
        let model = make_model(&cfg.base_url);
        let mut rx = api.stream_simple(cfg, &model, &user_ctx(), options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    async fn collect_with_options(
        cfg: &ProviderConfig,
        options: &PiMessagesOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = PiMessages;
        let model = make_model(&cfg.base_url);
        let mut rx = api.stream_with_options(cfg, &model, &user_ctx(), options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
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

    fn error_of(events: &[AssistantMessageEvent]) -> &AssistantMessage {
        match events.last().expect("terminal event") {
            AssistantMessageEvent::Error { error, .. } => error,
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // ---- 1. streams text and tool calls and resolves the terminal message
    // (pi-messages.test.ts:98-166) ----

    #[tokio::test]
    async fn streams_text_and_tool_calls_and_resolves_the_terminal_message() {
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse(&[
                json!({"type": "start"}),
                json!({"type": "text_start", "contentIndex": 0}),
                json!({"type": "text_delta", "contentIndex": 0, "delta": "Hel"}),
                json!({"type": "text_delta", "contentIndex": 0, "delta": "lo"}),
                json!({"type": "text_end", "contentIndex": 0, "content": "Hello"}),
                json!({"type": "toolcall_start", "contentIndex": 1, "id": "call_1", "toolName": "read"}),
                json!({"type": "toolcall_delta", "contentIndex": 1, "delta": "{\"path\":"}),
                json!({"type": "toolcall_delta", "contentIndex": 1, "delta": "\"a.txt\"}"}),
                json!({
                    "type": "toolcall_end",
                    "contentIndex": 1,
                    "toolCall": {"type": "toolCall", "id": "call_1", "name": "read", "arguments": {"path": "a.txt"}}
                }),
                json!({
                    "type": "done",
                    "reason": "toolUse",
                    "usage": usage_fixture(),
                    "responseId": "resp_1",
                    "providerThinkingLevel": "high"
                }),
            ]),
        )
        .await;

        let options = SimpleStreamOptions {
            stream: StreamOptions {
                api_key: Some("test-key".to_string()),
                session_id: Some("session-1".to_string()),
                max_tokens: Some(100),
                headers: Some(
                    [("x-custom".to_string(), Some("1".to_string()))]
                        .into_iter()
                        .collect(),
                ),
                ..StreamOptions::default()
            },
            tool_choice: Some(ToolChoice::Auto),
            ..SimpleStreamOptions::default()
        };
        let events = collect_simple(&keyed_cfg(&server), &options).await;

        // partialStopReasons[0] === "pending" (oracle line 142): after the
        // start event the reconstructed partial is still pending.
        assert_eq!(events[0].event_type(), "start");
        let mut after_start = PartialAssistant::new();
        after_start.apply(&events[0]).unwrap();
        assert_eq!(
            after_start
                .message()
                .expect("start seeds the partial")
                .stop_reason,
            StopReason::Pending
        );
        assert!(events
            .iter()
            .any(|event| event.event_type() == "text_delta"));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type() == "toolcall_end")
                .count(),
            1
        );

        let AssistantMessageEvent::Done {
            reason: SuccessReason::ToolUse,
            message,
        } = events.last().expect("terminal event")
        else {
            panic!("expected done, got {:?}", events.last());
        };
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(message.usage, expected_usage());
        assert_eq!(message.response_id.as_deref(), Some("resp_1"));
        assert_eq!(message.provider_thinking_level.as_deref(), Some("high"));
        assert_eq!(message.model, "auto");
        assert_eq!(message.provider, "radius");
        assert_eq!(
            message.content,
            vec![
                AssistantBlock::Text(TextContent {
                    text: "Hello".to_string(),
                    text_signature: None,
                }),
                AssistantBlock::ToolCall(ToolCall {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({"path": "a.txt"}),
                    thought_signature: None,
                    namespace: None,
                }),
            ]
        );
        // The reducer's final message is exactly the done event's message.
        let partial = apply_all(&events);
        assert_eq!(partial.message(), Some(message));

        // Request wire checks (oracle lines 156-165).
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.url.path(), "/v1/messages");
        let headers = &request.headers;
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer test-key"
        );
        assert_eq!(headers.get("x-custom").unwrap().to_str().unwrap(), "1");
        assert_eq!(
            headers.get("accept").unwrap().to_str().unwrap(),
            "text/event-stream"
        );
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "auto",
                "context": {"messages": [{"role": "user", "content": "Hello", "timestamp": TS}]},
                "options": {"maxTokens": 100, "sessionId": "session-1", "toolChoice": "auto"}
            })
        );
    }

    // ---- 2. appends debug=1 (oracle lines 168-188; the onResponse hook
    // assertion is not portable — the port has no response callback yet) ----

    #[tokio::test]
    async fn appends_debug_query_parameter() {
        let server = wiremock::MockServer::start().await;
        // Only a request carrying ?debug=1 matches this mock; anything else
        // gets wiremock's default 404 and fails the stream.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .and(wiremock::matchers::query_param("debug", "1"))
            .respond_with(sse(&[json!({
                "type": "done", "reason": "stop", "usage": usage_fixture()
            })]))
            .mount(&server)
            .await;

        // debug is an API-specific option (upstream PiMessagesOptions.debug),
        // so this drives the direct-options entry point.
        let options = PiMessagesOptions {
            stream: StreamOptions {
                api_key: Some("test-key".to_string()),
                ..StreamOptions::default()
            },
            debug: true,
            ..PiMessagesOptions::default()
        };
        let cfg = keyed_cfg(&server);
        let events = collect_with_options(&cfg, &options).await;

        match events.last().expect("terminal event") {
            AssistantMessageEvent::Done {
                reason: SuccessReason::Stop,
                message,
            } => {
                assert_eq!(message.stop_reason, StopReason::Stop);
                assert_eq!(message.usage, expected_usage());
            }
            other => panic!("expected done, got {other:?}"),
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.query(), Some("debug=1"));
    }

    // ---- 3. surfaces backend error responses with diagnostics
    // (pi-messages.test.ts:190-205) ----

    #[tokio::test]
    async fn surfaces_backend_error_responses_with_diagnostics() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(
                wiremock::ResponseTemplate::new(401)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        json!({"error": {"message": "Token expired", "code": "unauthorized"}})
                            .to_string(),
                    ),
            )
            .mount(&server)
            .await;

        let events = collect_stream(
            &server,
            &StreamOptions {
                api_key: Some("stale".to_string()),
                ..StreamOptions::default()
            },
        )
        .await;

        let message = error_of(&events);
        assert_eq!(message.stop_reason, StopReason::Error);
        let error_message = message.error_message.as_deref().unwrap_or_default();
        assert!(error_message.contains("401"), "{error_message}");
        assert!(error_message.contains("Token expired"), "{error_message}");
        assert!(error_message.contains("unauthorized"), "{error_message}");

        let diagnostics = message.diagnostics.as_ref().expect("diagnostics present");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].r#type, "pi_messages_response_failure");
        let details = diagnostics[0].details.as_ref().expect("details present");
        assert_eq!(details.get("status"), Some(&json!(401)));
        assert_eq!(details.get("statusText"), Some(&json!("Unauthorized")));
        assert_eq!(
            details.pointer("/error/message"),
            Some(&json!("Token expired"))
        );
        let error_info = diagnostics[0].error.as_ref().expect("error info present");
        assert_eq!(error_info.name.as_deref(), Some("PiMessagesResponseError"));
        assert_eq!(
            error_info.code,
            Some(DiagnosticCode::String("unauthorized".to_string()))
        );
    }

    // ---- 4. propagates server-sent error events (pi-messages.test.ts:207-218) ----

    #[tokio::test]
    async fn propagates_server_sent_error_events() {
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse(&[
                json!({"type": "start"}),
                json!({
                    "type": "error",
                    "reason": "error",
                    "usage": usage_fixture(),
                    "errorMessage": "Upstream failed"
                }),
            ]),
        )
        .await;

        let events = collect_stream(
            &server,
            &StreamOptions {
                api_key: Some("test-key".to_string()),
                ..StreamOptions::default()
            },
        )
        .await;

        match events.last().expect("terminal event") {
            AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error,
            } => {
                assert_eq!(error.stop_reason, StopReason::Error);
                assert_eq!(error.error_message.as_deref(), Some("Upstream failed"));
                assert_eq!(error.usage, expected_usage());
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    // ---- 5. errors when no API key is provided (pi-messages.test.ts:220-227) ----

    #[tokio::test]
    async fn errors_when_no_api_key_is_provided() {
        let server = wiremock::MockServer::start().await;
        let cfg = empty_cfg(&server);
        let model = make_model(&cfg.base_url);
        let api = PiMessages;
        let mut rx = api.stream(&cfg, &model, &user_ctx(), &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        let message = error_of(&events);
        assert_eq!(message.stop_reason, StopReason::Error);
        let error_message = message.error_message.as_deref().unwrap_or_default();
        assert!(
            error_message.contains("No API key provided"),
            "{error_message}"
        );
    }

    // ---- 6. errors when the stream ends without a terminal event
    // (pi-messages.test.ts:229-243) ----

    #[tokio::test]
    async fn errors_when_the_stream_ends_without_a_terminal_event() {
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse(&[
                json!({"type": "start"}),
                json!({"type": "text_start", "contentIndex": 0}),
                json!({"type": "text_delta", "contentIndex": 0, "delta": "partial"}),
            ]),
        )
        .await;

        let events = collect_stream(
            &server,
            &StreamOptions {
                api_key: Some("test-key".to_string()),
                ..StreamOptions::default()
            },
        )
        .await;

        let message = error_of(&events);
        assert_eq!(message.stop_reason, StopReason::Error);
        let error_message = message.error_message.as_deref().unwrap_or_default();
        assert!(
            error_message.contains("stream ended without a terminal event"),
            "{error_message}"
        );
    }

    // ---- 7. thinking blocks with signatures and the rewrite diagnostic
    // (typed upstream behavior: pi-messages.ts:232-243, 169-178, 71-77) ----

    #[tokio::test]
    async fn thinking_stream_with_signature_and_rewrite_diagnostic() {
        let server = wiremock::MockServer::start().await;
        let rewrite = json!({
            "policyId": "policy-1",
            "policyVersion": 2,
            "changed": true,
            "tokenCountChange": -5,
            "messageCountChange": 0,
            "systemPromptChanged": false
        });
        mount_sse(
            &server,
            sse(&[
                json!({"type": "start"}),
                json!({"type": "thinking_start", "contentIndex": 0}),
                json!({"type": "thinking_delta", "contentIndex": 0, "delta": "deep "}),
                json!({"type": "thinking_delta", "contentIndex": 0, "delta": "thought"}),
                json!({
                    "type": "thinking_end",
                    "contentIndex": 0,
                    "content": "deep thought",
                    "contentSignature": "sig-1",
                    "redacted": true
                }),
                json!({
                    "type": "done",
                    "reason": "stop",
                    "usage": usage_fixture(),
                    "rewrite": rewrite
                }),
            ]),
        )
        .await;

        let events = collect_stream(
            &server,
            &StreamOptions {
                api_key: Some("test-key".to_string()),
                ..StreamOptions::default()
            },
        )
        .await;

        let AssistantMessageEvent::Done {
            reason: SuccessReason::Stop,
            message,
        } = events.last().expect("terminal event")
        else {
            panic!("expected done, got {:?}", events.last());
        };
        assert_eq!(
            message.content,
            vec![AssistantBlock::Thinking(ThinkingContent {
                thinking: "deep thought".to_string(),
                thinking_signature: Some("sig-1".to_string()),
                redacted: Some(true),
            })]
        );
        // The rewrite diagnostic rides on the final message.
        let diagnostics = message.diagnostics.as_ref().expect("diagnostics present");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].r#type, "pi_messages_rewrite");
        assert_eq!(diagnostics[0].details.as_ref(), Some(&rewrite));
        // The reducer reconstructs the same message.
        assert_eq!(apply_all(&events).message(), Some(message));
    }

    // ---- 8. CRLF framing and the trailing unterminated data line
    // (pi-messages.ts:285-307) ----

    #[tokio::test]
    async fn parses_crlf_framing_and_unterminated_trailing_event() {
        let server = wiremock::MockServer::start().await;
        let mut body = String::new();
        for event in [
            json!({"type": "start"}),
            json!({"type": "text_start", "contentIndex": 0}),
            json!({"type": "text_delta", "contentIndex": 0, "delta": "Hel"}),
            json!({"type": "text_delta", "contentIndex": 0, "delta": "lo"}),
            json!({"type": "text_end", "contentIndex": 0, "content": "Hello"}),
            // The final done event has no trailing blank line.
            json!({
                "type": "done",
                "reason": "stop",
                "usage": usage_fixture(),
                "responseId": "resp_crlf"
            }),
        ] {
            body.push_str(&format!("data: {event}\r\n\r\n"));
        }
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;

        let events = collect_stream(
            &server,
            &StreamOptions {
                api_key: Some("test-key".to_string()),
                ..StreamOptions::default()
            },
        )
        .await;

        let AssistantMessageEvent::Done { message, .. } = events.last().expect("terminal event")
        else {
            panic!("expected done, got {:?}", events.last());
        };
        assert_eq!(message.response_id.as_deref(), Some("resp_crlf"));
        assert_eq!(
            message.content,
            vec![AssistantBlock::Text(TextContent {
                text: "Hello".to_string(),
                text_signature: None,
            })]
        );
    }

    // ---- 9. pure pieces ----

    #[test]
    fn tool_choice_wire_shapes_round_trip() {
        let cases: Vec<(&str, PiMessagesToolChoice)> = vec![
            (
                "\"auto\"",
                PiMessagesToolChoice::Named(PiMessagesNamedToolChoice::Auto),
            ),
            (
                "\"none\"",
                PiMessagesToolChoice::Named(PiMessagesNamedToolChoice::None),
            ),
            (
                "\"required\"",
                PiMessagesToolChoice::Named(PiMessagesNamedToolChoice::Required),
            ),
            (
                r#"{"type":"function","function":{"name":"read"}}"#,
                PiMessagesToolChoice::Function(PiMessagesFunctionChoice {
                    function_type: PiMessagesFunctionType::Function,
                    function: PiMessagesFunctionName {
                        name: "read".to_string(),
                    },
                }),
            ),
        ];
        for (wire, value) in cases {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(
                serde_json::from_str::<PiMessagesToolChoice>(wire).unwrap(),
                value
            );
        }
    }

    #[test]
    fn payload_omits_unset_option_fields() {
        let model = make_model("https://gateway.example/v1");
        let ctx = user_ctx();
        let payload = build_payload(&model, &ctx, &PiMessagesOptions::default());
        assert_eq!(
            payload,
            json!({
                "model": "auto",
                "context": {"messages": [{"role": "user", "content": "Hello", "timestamp": TS}]},
                "options": {}
            })
        );

        // Every extension set, including the object tool-choice form.
        let options = PiMessagesOptions {
            stream: StreamOptions {
                temperature: Some(0.5),
                max_tokens: Some(2048),
                cache_retention: Some(CacheRetention::None),
                session_id: Some("sess".to_string()),
                ..StreamOptions::default()
            },
            reasoning: Some(ThinkingLevel::High),
            tool_choice: Some(PiMessagesToolChoice::Function(PiMessagesFunctionChoice {
                function_type: PiMessagesFunctionType::Function,
                function: PiMessagesFunctionName {
                    name: "edit".to_string(),
                },
            })),
            debug: false,
        };
        let payload = build_payload(&model, &ctx, &options);
        assert_eq!(
            payload["options"],
            json!({
                "temperature": 0.5,
                "maxTokens": 2048,
                "reasoning": "high",
                "cacheRetention": "none",
                "sessionId": "sess",
                "toolChoice": {"type": "function", "function": {"name": "edit"}}
            })
        );
        assert_eq!(payload["model"], "auto");
    }

    #[test]
    fn options_from_simple_maps_reasoning_and_tool_choice() {
        let simple = SimpleStreamOptions {
            stream: StreamOptions {
                api_key: Some("sk".to_string()),
                ..StreamOptions::default()
            },
            tool_choice: Some(ToolChoice::None),
            reasoning: Some(ThinkingLevel::Medium),
            ..SimpleStreamOptions::default()
        };
        let options = options_from_simple(&simple);
        assert_eq!(options.stream.api_key.as_deref(), Some("sk"));
        assert_eq!(options.reasoning, Some(ThinkingLevel::Medium));
        assert_eq!(
            options.tool_choice,
            Some(PiMessagesToolChoice::Named(PiMessagesNamedToolChoice::None))
        );
        assert!(!options.debug);

        // from_stream keeps the extensions unset.
        let options = PiMessagesOptions::from_stream(StreamOptions::default());
        assert_eq!(options, PiMessagesOptions::default());
    }

    #[test]
    fn resolve_cache_retention_prefers_explicit_and_maps_long_env() {
        // An explicit value always wins, including `none`.
        assert_eq!(
            resolve_cache_retention(
                Some(CacheRetention::None),
                Some(
                    &[("PI_CACHE_RETENTION".to_string(), "long".to_string())]
                        .into_iter()
                        .collect()
                )
            ),
            Some(CacheRetention::None)
        );
        // The scoped env opt-in maps only `long`.
        let env: ProviderEnv = [("PI_CACHE_RETENTION".to_string(), "long".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            resolve_cache_retention(None, Some(&env)),
            Some(CacheRetention::Long)
        );
        let other: ProviderEnv = [("PI_CACHE_RETENTION".to_string(), "short".to_string())]
            .into_iter()
            .collect();
        assert_eq!(resolve_cache_retention(None, Some(&other)), None);
    }

    #[test]
    fn error_body_parsing_and_formatting_match_upstream() {
        // Non-JSON and JSON-without-error-object bodies yield no error body.
        assert!(parse_error_body("Internal Server Error").is_none());
        assert!(parse_error_body(r#"{"message":"nope"}"#).is_none());
        assert!(parse_error_body(r#"{"error":[1]}"#).is_none());

        let failure = create_response_failure(
            &make_model("https://gateway.example/v1"),
            "https://gateway.example/messages",
            401,
            "Unauthorized",
            r#"{"error":{"message":"Token expired","code":"unauthorized"}}"#,
        );
        assert_eq!(
            failure.message,
            "401 Unauthorized: Token expired (unauthorized)"
        );
        assert_eq!(failure.code.as_deref(), Some("unauthorized"));
        assert_eq!(failure.diagnostic_details.get("status"), Some(&json!(401)));
        assert_eq!(
            failure.diagnostic_details.pointer("/error/code"),
            Some(&json!("unauthorized"))
        );

        // A body without a usable error object falls back to the raw body as
        // the message suffix and into the truncated `body` detail.
        let failure = create_response_failure(
            &make_model("https://gateway.example/v1"),
            "https://gateway.example/messages",
            502,
            "Bad Gateway",
            "upstream exploded",
        );
        assert_eq!(failure.message, "502 Bad Gateway: upstream exploded");
        assert_eq!(failure.code, None);
        assert_eq!(
            failure.diagnostic_details.get("body"),
            Some(&json!("upstream exploded"))
        );
        assert!(failure.diagnostic_details.get("error").is_none());
    }

    #[test]
    fn truncate_diagnostic_string_appends_ellipsis_past_the_cap() {
        let short = "x".repeat(8192);
        assert_eq!(truncate_diagnostic_string(&short), short);
        let long = format!("{short}tail");
        let truncated = truncate_diagnostic_string(&long);
        assert_eq!(truncated.chars().count(), 8193);
        assert!(truncated.ends_with('\u{2026}'));
        assert!(!truncated.contains("tail"));
    }

    // ---- abort surface ----

    /// A pre-cancelled signal fails the request setup (the retry seam's
    /// `"Request aborted"` abort error) before `Start`, and the catch block
    /// settles the `aborted` reason (pi-messages.ts:424).
    #[tokio::test]
    async fn pre_aborted_request_settles_aborted_before_start() {
        let server = wiremock::MockServer::start().await;
        let token = CancellationToken::new();
        token.cancel();
        let cfg = keyed_cfg(&server);
        let options = PiMessagesOptions::from_stream(StreamOptions {
            signal: Some(token),
            ..StreamOptions::default()
        });
        let api = PiMessages;
        let model = make_model(&cfg.base_url);
        let mut rx = api.stream_with_options(&cfg, &model, &user_ctx(), &options);
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

    /// A cancellation after `start` breaks the body read and settles the
    /// `aborted` reason (upstream `readPiMessagesEvents` reads with the
    /// request signal attached). The raw TCP server writes the response head
    /// plus the wire `start` event and then stalls, so only the abort path
    /// can end the stream.
    #[tokio::test]
    async fn mid_stream_cancellation_settles_the_stream_aborted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                )
                .await;
            let _ = socket.write_all(b"data: {\"type\":\"start\"}\n\n").await;
            // Hold the socket open (no further bytes) until the runtime
            // drops the task.
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
        let cfg = ProviderConfig {
            base_url: format!("http://{addr}"),
            api_key: "cfg-key".to_string(),
            max_tokens: 16384,
        };
        let token = CancellationToken::new();
        let options = PiMessagesOptions::from_stream(StreamOptions {
            signal: Some(token.clone()),
            ..StreamOptions::default()
        });
        let api = PiMessages;
        let model = make_model(&cfg.base_url);
        let mut rx = api.stream_with_options(&cfg, &model, &user_ctx(), &options);
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
