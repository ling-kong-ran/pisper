//! Anthropic-messages streaming — full port of the stream/streamSimple
//! implementations from upstream `packages/ai/src/api/anthropic-messages.ts`
//! (lines 511-831): the raw SSE pipeline (`iterateSseMessages`/
//! `iterateAnthropicEvents`, lines 319-509) with its `error`-event and
//! message_stop framing rules, block accumulation over
//! content_block_start/delta/stop (lines 626-751), message_start/message_delta
//! usage capture with the 1h cache-write split and fallback-model cost
//! attribution (lines 602-625, 752-788), stop-reason mapping with raw
//! preservation (`mapStopReason`, lines 1494-1520), the
//! `anthropic_input_transformations` diagnostic (lines 801-813), and
//! pre-start vs mid-stream error handling (catch block, lines 817-827).
//! `streamSimple` (lines 858-904) is the option shaping in
//! [`options_from_simple`](crate::ai::api::anthropic::request::options_from_simple)
//! feeding the same loop.
//!
//! Deviations from upstream, all structural:
//! - Upstream events carry the live `partial`; the port emits events without
//!   it and consumers reconstruct via `PartialAssistant` (the M2a contract).
//!   The visible consequence: content_block_start initial content
//!   (`text`/`thinking`/`signature`, tool `input`) is not observable in the
//!   reconstructed partial until the authoritative `*_end` event — the gap the
//!   M2a event protocol already documents for tool-call arguments.
//! - `options.signal` is the port's non-serialized
//!   [`CancellationToken`](tokio_util::sync::CancellationToken): a
//!   pre-cancelled token or cancellation during the initial request fails
//!   before `Start` with the retry seam's `"Request aborted"` abort error and
//!   is never retried; a cancellation after `Start` breaks the SSE read (the
//!   `iterateSseMessages` abort check) and the catch block settles
//!   `stopReason: "aborted"` with `"Request was aborted"`.
//! - Process-local onPayload runs before SDK serialization and request retry;
//!   onResponse is awaited after successful HTTP headers and before Start.
//!   SDK-only params become headers and replacements force stream:true.
//!   Mid-flight stream errors and callback rejections are never retried.
//! - HTTP error bodies are surfaced as `"{status}: {body}"` (the shared
//!   `format_http_error` composition from the openai-completions port);
//!   upstream delegates to the Anthropic SDK's `APIError` message, whose exact
//!   composition is not pinned by any oracle test.
//! - The `Could not parse Anthropic SSE event ...` message includes the event
//!   name and data but not the upstream `raw=` segment: `eventsource_stream`
//!   does not expose the raw SSE lines the custom decoder retains.
//! - A `message_start` without a `usage` object maps to zeroed usage where
//!   upstream throws a TypeError on the missing-field access.
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order — same documented deviation as the request builder.

use std::collections::HashMap;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::ai::api::anthropic::request::{
    get_anthropic_compat, is_oauth_token, options_from_simple, prepare_request, resolve_api_key,
    sdk_request, spread_params, AnthropicCompat, AnthropicEffort, AnthropicOptions,
    RequestAssembly,
};
use crate::ai::api::openai_completions::stream::{format_http_error, parse_streaming_json};
use crate::ai::api::{http_client, request_signal, ApiImpl, REQUEST_WAS_ABORTED};
use crate::ai::cost::calculate_cost;
use crate::ai::json_parse::parse_json_with_repair;
use crate::ai::retry::{retry_provider_request, ProviderError};
use crate::ai::transcript::{get_current_tools, resolve_transcript, TranscriptContext};
use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::message::{AssistantBlock, AssistantMessage, AssistantMessageDiagnostic};
use crate::ai::types::options::{SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{ModelCost, StopReason, Usage};
use crate::ai::types::tool::Tool;
use crate::ai::types::Model;
use crate::ai::{now_ms, ProviderConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The API id stamped on every emitted message.
const API: &str = "anthropic-messages";

/// Upstream `ANTHROPIC_MESSAGE_EVENTS` (lines 331-338): the SSE event names
/// that carry protocol JSON; every other name (including the `error` name
/// handled before the filter) is skipped without parsing its data.
const ANTHROPIC_MESSAGE_EVENTS: [&str; 6] = [
    "message_start",
    "message_delta",
    "message_stop",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
];

pub struct AnthropicMessages;

impl ApiImpl for AnthropicMessages {
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
        // Upstream `stream` takes the API-specific option extension on top of
        // the base options; the port's `StreamOptions` is the base set, so the
        // extension fields stay unset for direct `stream` calls.
        let options = AnthropicOptions::from_stream(options.clone());
        run_stream(cfg.clone(), model.clone(), ctx.clone(), options)
    }

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream `streamSimple` (lines 858-904): shape the thinking/effort
        // options, then delegate to `stream`.
        let options = options_from_simple(model, ctx, options);
        run_stream(cfg.clone(), model.clone(), ctx.clone(), options)
    }
}

fn run_stream(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: AnthropicOptions,
) -> mpsc::Receiver<AssistantMessageEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        run_stream_task(cfg, model, ctx, options, tx).await;
    });
    rx
}

/// Accumulation state for one stream, the port of the upstream loop's locals
/// (`blocks` with their wire `index` fields, `partialJson` scratch,
/// `inputTransformations`, `usageModel`).
struct StreamState {
    output: AssistantMessage,
    /// Wire content-block index -> position in `output.content` (upstream
    /// stores `index` on each block and finds blocks by it). Removed at
    /// content_block_stop like upstream's `delete block.index` (line 724), so
    /// late deltas/stops for a stopped block find nothing.
    by_wire: HashMap<u64, usize>,
    /// Upstream `partialJson` scratch per tool-call block, keyed by content
    /// position (the M2a multi-accumulator fix: one buffer per block, not one
    /// shared buffer).
    tool_json: HashMap<usize, String>,
    /// Upstream `inputTransformations`: an `input_transformations` array on
    /// message_start or message_delta replaces the previous value.
    input_transformations: Option<Vec<Value>>,
}

impl StreamState {
    fn new(model: &Model, compat: &AnthropicCompat, options: &AnthropicOptions) -> Self {
        // Upstream lines 521-528: managed-effort models stamp the active
        // effort level (`options?.effort ?? "high"`) on the message.
        let provider_thinking_level = compat.supports_mid_convo_effort.then(|| {
            options
                .effort
                .map_or("high", AnthropicEffort::as_str)
                .to_string()
        });
        StreamState {
            output: AssistantMessage {
                content: Vec::new(),
                api: API.to_string(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                provider_thinking_level,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: now_ms(),
            },
            by_wire: HashMap::new(),
            tool_json: HashMap::new(),
            input_transformations: None,
        }
    }
}

/// Upstream `fromClaudeCodeName` (lines 116-123): map a Claude Code tool name
/// back to the declared tool's name for OAuth requests.
fn from_claude_code_name(name: &str, tools: &[Tool]) -> String {
    if !tools.is_empty() {
        let lowered = name.to_lowercase();
        if let Some(tool) = tools
            .iter()
            .find(|tool| tool.name.to_lowercase() == lowered)
        {
            return tool.name.clone();
        }
    }
    name.to_string()
}

/// JS `value || 0` for wire token counts: absent/null/non-numeric yield 0.
fn token_count(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(number)) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|value| value.max(0.0) as u64))
            .unwrap_or(0),
        _ => 0,
    }
}

/// Send the assembled request (upstream
/// `client.beta.messages.create(...).asResponse()`), wrapped in the provider
/// retry policy with `options.stream.max_retries`/`max_retry_delay_ms`
/// (upstream invokes the SDK with `maxRetries: 0` and wraps
/// `retryProviderRequest` around the call). Retries cover transport failures
/// and retryable statuses only — the SDK throws before the response stream is
/// returned, so once stream bytes flow an error is never retried. A
/// cancelled signal fails the request with the abort error (upstream
/// `createAbortError`), never retried.
async fn send_stream_request(
    cfg: &ProviderConfig,
    assembly: &RequestAssembly,
    options: &AnthropicOptions,
    signal: &CancellationToken,
) -> Result<reqwest::Response, String> {
    if signal.is_cancelled() {
        return Err(crate::ai::api::REQUEST_ABORTED.into());
    }
    // SDK preflight errors are plain errors, not retryable ProviderErrors.
    // Materialize its deterministic transformation once before HTTP retries.
    let assembly = sdk_request(assembly)?;
    let url = format!(
        "{}/v1/messages?beta=true",
        cfg.base_url.trim_end_matches('/')
    );
    // The assembly headers carry the full SDK header set including the
    // injected auth pair (`x-api-key` / `Authorization`) and
    // `anthropic-version`.
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in &assembly.headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("Invalid header name \"{name}\": {error}"))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|error| format!("Invalid header value for \"{name}\": {error}"))?;
        headers.insert(name, value);
    }
    let mut request = http_client()
        .post(&url)
        .headers(headers)
        .json(&assembly.body);
    if let Some(ms) = options.stream.timeout_ms {
        request = request.timeout(Duration::from_millis(ms));
    }
    let max_retries = options.stream.max_retries.unwrap_or(0);
    let max_retry_delay_ms = options.stream.max_retry_delay_ms;
    retry_provider_request(max_retries, max_retry_delay_ms, Some(signal), || async {
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
        Err(ProviderError::http(
            status_code,
            response_headers,
            format_http_error(status_code, &body),
        ))
    })
    .await
    .map_err(|error| error.message)
}

async fn run_stream_task(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: AnthropicOptions,
    tx: mpsc::Sender<AssistantMessageEvent>,
) {
    let compat = get_anthropic_compat(&model);
    let mut state = StreamState::new(&model, &compat, &options);
    let signal = request_signal(&options.stream.signal);
    match drive_stream(&cfg, &model, &ctx, &options, &signal, &mut state, &tx).await {
        Ok(()) => {}
        Err(message) => {
            // Upstream catch block (lines 817-827): the partial message keeps
            // its content; `stopReason` settles to "aborted" when the request
            // signal fired, else "error", and the thrown value becomes
            // `errorMessage`.
            let aborted = signal.is_cancelled();
            state.output.stop_reason = if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            state.output.error_message = Some(message);
            let _ = tx
                .send(AssistantMessageEvent::Error {
                    reason: if aborted {
                        ErrorReason::Aborted
                    } else {
                        ErrorReason::Error
                    },
                    error: state.output.clone(),
                })
                .await;
        }
    }
}

async fn drive_stream(
    cfg: &ProviderConfig,
    model: &Model,
    ctx: &TranscriptContext,
    options: &AnthropicOptions,
    signal: &CancellationToken,
    state: &mut StreamState,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let compat = get_anthropic_compat(model);
    // Upstream lines 517-518: the transcript is resolved once up front and the
    // current tool list is captured for the OAuth reverse name mapping.
    let normalized =
        resolve_transcript(ctx.clone(), Some(compat.supports_mid_convo_system_messages));
    let current_tools = get_current_tools(normalized.messages());
    // Upstream lines 547-576: credential resolution and the OAuth flag
    // (`apiKey && isOAuthToken(apiKey)`; header-owned auth is not OAuth).
    let api_key = resolve_api_key(model, cfg, options)?;
    let copilot = model.provider == "github-copilot";
    let is_oauth = !copilot && api_key.as_deref().is_some_and(is_oauth_token);
    let mut assembly = prepare_request(model, cfg, &normalized, options)?;
    // Upstream awaits both hooks without racing the signal. A cancellation
    // during onPayload is observed by the request seam after the hook returns.
    // `undefined` preserves params; even null is a real replacement, spread
    // into an object with stream:true. This differs from OpenAI Completions.
    if let Some(callback) = &options.stream.callbacks.on_payload {
        if let Some(replacement) = callback(assembly.body.clone(), model.clone())
            .await
            .map_err(|error| error.to_string())?
        {
            let mut params = spread_params(replacement)?;
            params.insert("stream".into(), Value::Bool(true));
            assembly.body = Value::Object(params);
        }
    }

    let response = send_stream_request(cfg, &assembly, options, signal).await?;
    options
        .stream
        .callbacks
        .http_response(&response, model)
        .await
        .map_err(|error| error.to_string())?;

    // Upstream line 596: `start` after the response arrives, before any event.
    let _ = tx
        .send(AssistantMessageEvent::Start {
            message: state.output.clone(),
        })
        .await;

    let mut usage_model = model.clone();
    let mut saw_message_start = false;
    let mut saw_message_stop = false;
    let mut events = response.bytes_stream().eventsource();
    // Upstream `iterateAnthropicEvents`/`iterateSseMessages` check
    // `signal?.aborted` on every read ("Request was aborted"); the select
    // breaks the read the moment the token cancels.
    loop {
        let event = tokio::select! {
            biased;
            _ = signal.cancelled() => return Err(REQUEST_WAS_ABORTED.to_string()),
            item = events.next() => match item {
                Some(item) => item.map_err(|error| error.to_string())?,
                None => break,
            },
        };
        // Upstream lines 482-484: an `error` SSE event throws with the raw
        // data payload as the message.
        if event.event == "error" {
            return Err(event.data);
        }
        // Upstream lines 486-488: non-protocol events are skipped before any
        // JSON parsing.
        if !ANTHROPIC_MESSAGE_EVENTS.contains(&event.event.as_str()) {
            continue;
        }
        let data = parse_json_with_repair(&event.data).map_err(|error| {
            format!(
                "Could not parse Anthropic SSE event {}: {error}; data={}",
                event.event, event.data
            )
        })?;
        match data.get("type").and_then(Value::as_str) {
            Some("message_start") => saw_message_start = true,
            Some("message_stop") => saw_message_stop = true,
            _ => {}
        }
        // Upstream: the per-event observer runs before Pi normalization.
        options
            .stream
            .callbacks
            .provider_stream_event(data.clone(), model)
            .await
            .map_err(|error| error.to_string())?;
        process_event(
            state,
            &data,
            model,
            &mut usage_model,
            is_oauth,
            &current_tools,
            tx,
        )
        .await?;
    }

    // Upstream line 791: the post-loop abort check precedes every other
    // terminal guard.
    if signal.is_cancelled() {
        return Err(REQUEST_WAS_ABORTED.to_string());
    }
    // Upstream lines 506-508: the iterator throws when the stream ends after
    // message_start without message_stop.
    if saw_message_start && !saw_message_stop {
        return Err("Anthropic stream ended before message_stop".to_string());
    }
    // Upstream lines 795-800.
    if state.output.stop_reason == StopReason::Pending {
        return Err("Anthropic stream ended without a stop reason".to_string());
    }
    if matches!(
        state.output.stop_reason,
        StopReason::Aborted | StopReason::Error
    ) {
        return Err(state
            .output
            .error_message
            .clone()
            .unwrap_or_else(|| "An unknown error occurred".to_string()));
    }

    // Upstream lines 801-813: the transformations diagnostic is appended on
    // the success path only, before `done`.
    append_input_transformations_diagnostic(
        &mut state.output,
        state.input_transformations.as_deref(),
    );

    let _ = tx
        .send(AssistantMessageEvent::Done {
            reason: success_reason(state.output.stop_reason),
            message: state.output.clone(),
        })
        .await;
    Ok(())
}

fn success_reason(stop_reason: StopReason) -> SuccessReason {
    match stop_reason {
        StopReason::Length => SuccessReason::Length,
        StopReason::ToolUse => SuccessReason::ToolUse,
        _ => SuccessReason::Stop,
    }
}

/// One protocol SSE event (upstream lines 601-789). Dispatch is on the parsed
/// `type` field; unknown types are ignored.
#[allow(clippy::too_many_arguments)]
async fn process_event(
    state: &mut StreamState,
    event: &Value,
    model: &Model,
    usage_model: &mut Model,
    is_oauth: bool,
    current_tools: &[Tool],
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    match event.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            process_message_start(state, event, model, usage_model);
        }
        Some("content_block_start") => {
            process_content_block_start(state, event, is_oauth, current_tools, tx).await?;
        }
        Some("content_block_delta") => {
            process_content_block_delta(state, event, tx).await?;
        }
        Some("content_block_stop") => {
            process_content_block_stop(state, event, tx).await;
        }
        Some("message_delta") => {
            process_message_delta(state, event, usage_model)?;
        }
        // message_stop and unknown types carry no per-event work.
        _ => {}
    }
    Ok(())
}

/// Upstream lines 602-625: response id, model relabel, fallback cost
/// attribution, and the initial usage capture.
fn process_message_start(
    state: &mut StreamState,
    event: &Value,
    model: &Model,
    usage_model: &mut Model,
) {
    let message = &event["message"];
    state.output.response_id = message
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(transformations) = message
        .get("input_transformations")
        .filter(|value| value.is_array())
    {
        state.input_transformations = transformations.as_array().cloned();
    }
    let response_model = message.get("model").and_then(Value::as_str);
    state.output.response_model = match response_model {
        Some(response_model) if response_model != model.id => Some(response_model.to_string()),
        _ => None,
    };
    // Upstream lines 608-614: a relabeled serving model that matches an
    // `allowedFallbackModels` entry switches the cost model for calculateCost.
    let fallback_cost = match response_model {
        Some(response_model) if response_model == model.id => None,
        Some(response_model) => model
            .compat
            .as_ref()
            .and_then(|compat| compat.get("allowedFallbackModels"))
            .and_then(Value::as_array)
            .and_then(|fallbacks| {
                fallbacks.iter().find(|fallback| {
                    fallback.get("provider").and_then(Value::as_str)
                        == Some(model.provider.as_str())
                        && fallback.get("model").and_then(Value::as_str) == Some(response_model)
                })
            })
            .and_then(|fallback| fallback.get("cost"))
            .filter(|cost| cost.is_object())
            .and_then(|cost| serde_json::from_value::<ModelCost>(cost.clone()).ok()),
        None => None,
    };
    *usage_model = match (response_model, fallback_cost) {
        (Some(response_model), Some(cost)) => {
            let mut usage_model = model.clone();
            usage_model.id = response_model.to_string();
            usage_model.cost = cost;
            usage_model
        }
        _ => model.clone(),
    };
    // Upstream lines 615-624: the initial token counts are captured even if
    // the stream ends early. `|| 0` on every field; cacheWrite1h is always a
    // number after message_start.
    let usage = &message["usage"];
    let input = token_count(usage.get("input_tokens"));
    let output = token_count(usage.get("output_tokens"));
    let cache_read = token_count(usage.get("cache_read_input_tokens"));
    let cache_write = token_count(usage.get("cache_creation_input_tokens"));
    state.output.usage.input = input;
    state.output.usage.output = output;
    state.output.usage.cache_read = cache_read;
    state.output.usage.cache_write = cache_write;
    state.output.usage.cache_write_1h = Some(token_count(
        usage.pointer("/cache_creation/ephemeral_1h_input_tokens"),
    ));
    // Anthropic doesn't provide total_tokens; compute from components.
    // Saturating: a hostile usage payload with huge component values must not
    // trip the debug overflow check mid-stream.
    state.output.usage.total_tokens = input
        .saturating_add(output)
        .saturating_add(cache_read)
        .saturating_add(cache_write);
    calculate_cost(usage_model, &mut state.output.usage);
}

/// Upstream lines 626-673: block creation from content_block_start.
async fn process_content_block_start(
    state: &mut StreamState,
    event: &Value,
    is_oauth: bool,
    current_tools: &[Tool],
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let wire_index = event.get("index").and_then(Value::as_u64);
    let block = &event["content_block"];
    match block.get("type").and_then(Value::as_str) {
        // Upstream lines 627-632: a fallback is only legal before any content.
        Some("fallback") => {
            if !state.output.content.is_empty() {
                return Err(
                    "Anthropic performed an unsupported mid-output model fallback".to_string(),
                );
            }
        }
        Some("text") => {
            let content_index = state.output.content.len();
            state.output.content.push(AssistantBlock::Text(TextContent {
                text: block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                text_signature: None,
            }));
            if let Some(wire_index) = wire_index {
                state.by_wire.insert(wire_index, content_index);
            }
            let _ = tx
                .send(AssistantMessageEvent::TextStart { content_index })
                .await;
        }
        Some("thinking") => {
            let content_index = state.output.content.len();
            state
                .output
                .content
                .push(AssistantBlock::Thinking(ThinkingContent {
                    thinking: block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    thinking_signature: Some(
                        block
                            .get("signature")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    redacted: None,
                }));
            if let Some(wire_index) = wire_index {
                state.by_wire.insert(wire_index, content_index);
            }
            let _ = tx
                .send(AssistantMessageEvent::ThinkingStart { content_index })
                .await;
        }
        // Upstream lines 650-659: complete at start; content arrives through
        // the authoritative thinking_end in the port.
        Some("redacted_thinking") => {
            let content_index = state.output.content.len();
            state
                .output
                .content
                .push(AssistantBlock::Thinking(ThinkingContent {
                    thinking: "[Reasoning redacted]".to_string(),
                    thinking_signature: Some(
                        block
                            .get("data")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    redacted: Some(true),
                }));
            if let Some(wire_index) = wire_index {
                state.by_wire.insert(wire_index, content_index);
            }
            let _ = tx
                .send(AssistantMessageEvent::ThinkingStart { content_index })
                .await;
        }
        Some("tool_use") => {
            let content_index = state.output.content.len();
            let name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            state
                .output
                .content
                .push(AssistantBlock::ToolCall(ToolCall {
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: if is_oauth {
                        from_claude_code_name(name, current_tools)
                    } else {
                        name.to_string()
                    },
                    arguments: block
                        .get("input")
                        .filter(|value| !value.is_null())
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                    thought_signature: None,
                    namespace: None,
                }));
            if let Some(wire_index) = wire_index {
                state.by_wire.insert(wire_index, content_index);
                state.tool_json.insert(content_index, String::new());
            }
            let _ = tx
                .send(AssistantMessageEvent::ToolcallStart { content_index })
                .await;
        }
        // Unknown block types push no block and emit no event (line 673).
        _ => {}
    }
    Ok(())
}

/// The content position for a wire block index when it maps to a live block
/// matching `kind` (upstream: `blocks.findIndex((b) => b.index === event.index)`
/// plus the per-delta `block.type` guards).
fn live_block(
    state: &StreamState,
    wire_index: Option<u64>,
    kind: fn(&AssistantBlock) -> bool,
) -> Option<usize> {
    let content_index = state.by_wire.get(&wire_index?).copied()?;
    kind(state.output.content.get(content_index)?).then_some(content_index)
}

fn is_text_block(block: &AssistantBlock) -> bool {
    matches!(block, AssistantBlock::Text(_))
}

fn is_thinking_block(block: &AssistantBlock) -> bool {
    matches!(block, AssistantBlock::Thinking(_))
}

fn is_tool_call_block(block: &AssistantBlock) -> bool {
    matches!(block, AssistantBlock::ToolCall(_))
}

/// Upstream lines 674-719: content_block_delta handling.
async fn process_content_block_delta(
    state: &mut StreamState,
    event: &Value,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let wire_index = event.get("index").and_then(Value::as_u64);
    let delta = &event["delta"];
    match delta.get("type").and_then(Value::as_str) {
        Some("text_delta") => {
            let Some(delta_text) = delta.get("text").and_then(Value::as_str) else {
                return Ok(());
            };
            let Some(content_index) = live_block(state, wire_index, is_text_block) else {
                return Ok(());
            };
            if let Some(AssistantBlock::Text(text)) = state.output.content.get_mut(content_index) {
                text.text.push_str(delta_text);
            }
            let _ = tx
                .send(AssistantMessageEvent::TextDelta {
                    content_index,
                    delta: delta_text.to_string(),
                })
                .await;
        }
        Some("thinking_delta") => {
            let Some(delta_thinking) = delta.get("thinking").and_then(Value::as_str) else {
                return Ok(());
            };
            let Some(content_index) = live_block(state, wire_index, is_thinking_block) else {
                return Ok(());
            };
            if let Some(AssistantBlock::Thinking(thinking)) =
                state.output.content.get_mut(content_index)
            {
                thinking.thinking.push_str(delta_thinking);
            }
            let _ = tx
                .send(AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta: delta_thinking.to_string(),
                })
                .await;
        }
        Some("input_json_delta") => {
            let Some(fragment) = delta.get("partial_json").and_then(Value::as_str) else {
                return Ok(());
            };
            let Some(content_index) = live_block(state, wire_index, is_tool_call_block) else {
                return Ok(());
            };
            state
                .tool_json
                .entry(content_index)
                .or_default()
                .push_str(fragment);
            let partial_json = state
                .tool_json
                .get(&content_index)
                .cloned()
                .unwrap_or_default();
            let parsed = parse_streaming_json(&partial_json);
            if let Some(AssistantBlock::ToolCall(call)) =
                state.output.content.get_mut(content_index)
            {
                call.arguments = parsed;
            }
            let _ = tx
                .send(AssistantMessageEvent::ToolcallDelta {
                    content_index,
                    delta: fragment.to_string(),
                })
                .await;
        }
        // Upstream lines 712-719: the signature delta updates the thinking
        // block's signature in place and emits no event.
        Some("signature_delta") => {
            let Some(delta_signature) = delta.get("signature").and_then(Value::as_str) else {
                return Ok(());
            };
            let Some(content_index) = live_block(state, wire_index, is_thinking_block) else {
                return Ok(());
            };
            if let Some(AssistantBlock::Thinking(thinking)) =
                state.output.content.get_mut(content_index)
            {
                // Upstream `block.thinkingSignature = block.thinkingSignature || ""`.
                match &mut thinking.thinking_signature {
                    Some(signature) => signature.push_str(delta_signature),
                    None => thinking.thinking_signature = Some(delta_signature.to_string()),
                }
            }
        }
        // Unknown delta types are ignored.
        _ => {}
    }
    Ok(())
}

/// Upstream lines 720-751: content_block_stop closes the block with its
/// authoritative end event.
async fn process_content_block_stop(
    state: &mut StreamState,
    event: &Value,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    let wire_index = event.get("index").and_then(Value::as_u64);
    let content_index = wire_index.and_then(|index| state.by_wire.get(&index).copied());
    if let Some(content_index) = content_index {
        match state.output.content.get(content_index) {
            Some(AssistantBlock::Text(_)) => {
                let content = match &state.output.content[content_index] {
                    AssistantBlock::Text(text) => text.text.clone(),
                    _ => String::new(),
                };
                let _ = tx
                    .send(AssistantMessageEvent::TextEnd {
                        content_index,
                        content,
                    })
                    .await;
            }
            Some(AssistantBlock::Thinking(_)) => {
                let content = match &state.output.content[content_index] {
                    AssistantBlock::Thinking(thinking) => thinking.thinking.clone(),
                    _ => String::new(),
                };
                let _ = tx
                    .send(AssistantMessageEvent::ThinkingEnd {
                        content_index,
                        content,
                    })
                    .await;
            }
            Some(AssistantBlock::ToolCall(_)) => {
                // Upstream lines 739-749: parse the accumulated fragments and
                // finalize the tool call in place.
                let partial_json = state
                    .tool_json
                    .get(&content_index)
                    .cloned()
                    .unwrap_or_default();
                let arguments = parse_streaming_json(&partial_json);
                let mut tool_call = match &state.output.content[content_index] {
                    AssistantBlock::ToolCall(call) => call.clone(),
                    _ => return,
                };
                tool_call.arguments = arguments;
                state.output.content[content_index] = AssistantBlock::ToolCall(tool_call.clone());
                let _ = tx
                    .send(AssistantMessageEvent::ToolcallEnd {
                        content_index,
                        tool_call,
                    })
                    .await;
            }
            None => {}
        }
        state.tool_json.remove(&content_index);
    }
    // Upstream `delete block.index` (line 724): later deltas/stops for the
    // same wire index find nothing.
    if let Some(wire_index) = wire_index {
        state.by_wire.remove(&wire_index);
    }
}

/// Upstream lines 752-788: stop-reason mapping, usage updates, and the
/// cost recomputation that runs for every message_delta.
fn process_message_delta(
    state: &mut StreamState,
    event: &Value,
    usage_model: &Model,
) -> Result<(), String> {
    if let Some(transformations) = event
        .get("input_transformations")
        .filter(|value| value.is_array())
    {
        state.input_transformations = transformations.as_array().cloned();
    }
    let delta = &event["delta"];
    // Truthiness guard: absent or empty stop reasons change nothing.
    if let Some(stop_reason) = delta
        .get("stop_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    {
        state.output.raw_stop_reason = Some(stop_reason.to_string());
        let (stop_reason, error_message) = map_stop_reason(stop_reason, delta.get("stop_details"))?;
        state.output.stop_reason = stop_reason;
        if let Some(message) = error_message {
            state.output.error_message = Some(message);
        }
    }
    // Upstream lines 763-783: only update usage fields that are present (not
    // null), preserving the message_start counts when proxies omit them.
    if let Some(usage) = event.get("usage").filter(|value| value.is_object()) {
        if let Some(input) = usage.get("input_tokens").and_then(Value::as_u64) {
            state.output.usage.input = input;
        }
        if let Some(output) = usage.get("output_tokens").and_then(Value::as_u64) {
            state.output.usage.output = output;
        }
        if let Some(cache_read) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) {
            state.output.usage.cache_read = cache_read;
        }
        if let Some(cache_write) = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
        {
            state.output.usage.cache_write = cache_write;
        }
        // Vercel AI Gateway includes the TTL breakdown in deltas, though the
        // SDK only types it on message_start (the cacheWrite1h delta).
        if let Some(cache_creation_1h) = usage
            .pointer("/cache_creation/ephemeral_1h_input_tokens")
            .and_then(Value::as_u64)
        {
            state.output.usage.cache_write_1h = Some(cache_creation_1h);
        }
        // Anthropic reports reasoning tokens as a subset of output tokens.
        if let Some(thinking_tokens) = usage
            .pointer("/output_tokens_details/thinking_tokens")
            .and_then(Value::as_u64)
        {
            state.output.usage.reasoning = Some(thinking_tokens);
        }
    }
    // Saturating sum: huge component values (hostile usage payload) must not
    // trip the debug overflow check.
    state.output.usage.total_tokens = state
        .output
        .usage
        .input
        .saturating_add(state.output.usage.output)
        .saturating_add(state.output.usage.cache_read)
        .saturating_add(state.output.usage.cache_write);
    calculate_cost(usage_model, &mut state.output.usage);
    Ok(())
}

/// Upstream `mapStopReason` (lines 1494-1520). The default branch throws
/// upstream, which surfaces as `Err` here: the raw reason is already stored
/// and the error flows to the catch block with the partial content kept.
fn map_stop_reason(
    reason: &str,
    stop_details: Option<&Value>,
) -> Result<(StopReason, Option<String>), String> {
    let stop_reason = match reason {
        "end_turn" => StopReason::Stop,
        "max_tokens" => StopReason::Length,
        "tool_use" => StopReason::ToolUse,
        "refusal" => {
            let explanation = stop_details
                .and_then(|details| details.get("explanation"))
                .and_then(Value::as_str)
                .filter(|explanation| !explanation.is_empty())
                .unwrap_or("The model refused to complete the request");
            return Ok((StopReason::Error, Some(explanation.to_string())));
        }
        // "Stop is good enough -> resubmit" (line 1510).
        "pause_turn" => StopReason::Stop,
        // "We don't supply stop sequences, so this should never happen"
        // (line 1513).
        "stop_sequence" => StopReason::Stop,
        // Content flagged by safety filters (line 1514).
        "sensitive" => {
            return Ok((
                StopReason::Error,
                Some("Provider stopped with: sensitive".to_string()),
            ));
        }
        // Unknown stop reasons fail gracefully (API may add new values).
        other => return Err(format!("Unhandled stop reason: {other}")),
    };
    Ok((stop_reason, None))
}

/// Upstream lines 801-813: the `anthropic_input_transformations` diagnostic;
/// one entry per transformation with `type`/`path`/`reason` (absent or null
/// fields dropped, like the JS `?? undefined` mapping).
fn append_input_transformations_diagnostic(
    output: &mut AssistantMessage,
    transformations: Option<&[Value]>,
) {
    let Some(transformations) =
        transformations.filter(|transformations| !transformations.is_empty())
    else {
        return;
    };
    let mapped: Vec<Value> = transformations
        .iter()
        .map(|transformation| {
            let mut object = Map::new();
            for key in ["type", "path", "reason"] {
                if let Some(value) = transformation.get(key).filter(|value| !value.is_null()) {
                    object.insert(key.to_string(), value.clone());
                }
            }
            Value::Object(object)
        })
        .collect();
    let diagnostic = AssistantMessageDiagnostic {
        r#type: "anthropic_input_transformations".to_string(),
        timestamp: now_ms(),
        error: None,
        details: Some(json!({ "transformations": mapped })),
    };
    output
        .diagnostics
        .get_or_insert_with(Vec::new)
        .push(diagnostic);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::ai::api::{abort_test_support::stalled_sse_server, REQUEST_ABORTED};
    use serde_json::json;

    use crate::ai::api::anthropic::request::options_from_simple;
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::content::TextContent;
    use crate::ai::types::events::{ErrorReason, PartialAssistant, SuccessReason};
    use crate::ai::types::message::{AssistantBlock, Message, StringOrBlocks, UserMessage};
    use crate::ai::types::primitives::{ModelCost, StopReason, ThinkingLevel, Usage};
    use crate::ai::types::tool::Tool;

    const TS: i64 = 1758240000000;

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
            input: vec![crate::ai::types::ModelInput::Text],
            cost: ModelCost::default(),
            context_window: 200000,
            max_tokens: 32000,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: Some(compat),
        }
    }

    /// claude-opus-4-8 pricing (upstream anthropic-cache-write-1h-cost.test.ts:
    /// input 5, cacheWrite 6.25 per Mtok; 1h write = 2x input = 10).
    fn opus_priced_model(compat: Value) -> Model {
        let mut model = make_model(compat);
        model.cost = ModelCost {
            input: 5.0,
            output: 30.0,
            cache_read: 0.5,
            cache_write: 6.25,
            tiers: None,
        };
        model
    }

    fn managed_model() -> Model {
        make_model(json!({
            "forceAdaptiveThinking": true,
            "supportsMidConvoEffort": true
        }))
    }

    fn cfg(server: &wiremock::MockServer) -> ProviderConfig {
        ProviderConfig {
            base_url: server.uri(),
            api_key: "test-key".to_string(),
            max_tokens: 32000,
        }
    }

    fn user_ctx(messages: Vec<Message>) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: None,
            messages,
            tools: None,
        })
    }

    fn user_msg(text: &str) -> Message {
        Message::User(UserMessage {
            content: StringOrBlocks::Text(text.to_string()),
            timestamp: TS,
        })
    }

    fn tool_decl(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: format!("Tool {name}"),
            parameters: json!({"type": "object", "properties": {}}),
            constrained_sampling: None,
        }
    }

    fn edit_tool() -> Tool {
        Tool {
            name: "edit".to_string(),
            description: "Edit a file.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "text": {"type": "string"}},
                "required": ["path", "text"]
            }),
            constrained_sampling: None,
        }
    }

    fn tools_ctx(messages: Vec<Message>, tools: Vec<Tool>) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: None,
            messages,
            tools: Some(tools),
        })
    }

    // ---- SSE helpers ----

    fn sse(body: &str) -> wiremock::ResponseTemplate {
        wiremock::ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body.to_string())
    }

    fn ev(name: &str, data: Value) -> String {
        raw_ev(name, &data.to_string())
    }

    fn raw_ev(name: &str, data: &str) -> String {
        format!("event: {name}\ndata: {data}\n\n")
    }

    fn message_start(message: Value) -> String {
        ev(
            "message_start",
            json!({"type": "message_start", "message": message}),
        )
    }

    fn start_usage() -> Value {
        json!({
            "input_tokens": 12,
            "output_tokens": 0,
            "cache_read_input_tokens": 0,
            "cache_creation_input_tokens": 0
        })
    }

    fn delta_usage() -> Value {
        json!({
            "input_tokens": 12,
            "output_tokens": 5,
            "cache_read_input_tokens": 0,
            "cache_creation_input_tokens": 0
        })
    }

    /// Upstream `minimalAnthropicEvents` (anthropic-sse-parsing.test.ts:17-70).
    fn minimal_events() -> Vec<String> {
        vec![
            message_start(json!({"id": "msg_test", "usage": start_usage()})),
            ev(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            ),
            ev(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}),
            ),
            ev(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            ),
            ev(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": delta_usage()}),
            ),
            ev("message_stop", json!({"type": "message_stop"})),
        ]
    }

    fn sse_body(events: &[String]) -> String {
        events.concat()
    }

    async fn mount_sse(server: &wiremock::MockServer, body: String) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(sse(&body))
            .mount(server)
            .await;
    }

    // ---- abort surface ----

    /// A pre-cancelled signal fails the request setup (the retry seam's
    /// `"Request aborted"` abort error) before `Start`, and the catch block
    /// settles `stopReason: "aborted"` — upstream's pre-stream abort path.
    #[tokio::test]
    async fn pre_aborted_request_settles_aborted_before_start() {
        let server = wiremock::MockServer::start().await;
        let token = CancellationToken::new();
        token.cancel();
        let options = StreamOptions {
            signal: Some(token),
            ..StreamOptions::default()
        };
        let api = AnthropicMessages;
        let mut rx = api.stream(
            &cfg(&server),
            &make_model(Value::Null),
            &user_ctx(vec![user_msg("hi")]),
            &options,
        );
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

    /// A cancellation after `start` breaks the body wait and settles the
    /// stream aborted with `"Request was aborted"` (upstream
    /// `iterateSseMessages` abort check + catch block). The stalled server
    /// holds the response headers back from producing any event, so only the
    /// abort path can end the stream.
    #[tokio::test]
    async fn mid_stream_cancellation_settles_the_stream_aborted() {
        let base_url = stalled_sse_server().await;
        let cfg = ProviderConfig {
            base_url,
            api_key: "test-key".to_string(),
            max_tokens: 32000,
        };
        let token = CancellationToken::new();
        let options = StreamOptions {
            signal: Some(token.clone()),
            ..StreamOptions::default()
        };
        let api = AnthropicMessages;
        let mut rx = api.stream(
            &cfg,
            &make_model(Value::Null),
            &user_ctx(vec![user_msg("hi")]),
            &options,
        );
        // Start arrives once the (stalled) response headers do.
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

    async fn collect_stream(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = AnthropicMessages;
        let mut rx = api.stream(&cfg(server), model, ctx, options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    async fn collect_simple(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = AnthropicMessages;
        let mut rx = api.stream_simple(&cfg(server), model, ctx, options);
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

    fn event_types(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(AssistantMessageEvent::event_type)
            .collect()
    }

    fn done_of(events: &[AssistantMessageEvent]) -> (&SuccessReason, &AssistantMessage) {
        match events.last().expect("terminal event") {
            AssistantMessageEvent::Done { reason, message } => (reason, message),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    fn error_of(events: &[AssistantMessageEvent]) -> (&ErrorReason, &AssistantMessage) {
        match events.last().expect("terminal event") {
            AssistantMessageEvent::Error { reason, error } => (reason, error),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    fn text_block(text: &str) -> AssistantBlock {
        AssistantBlock::Text(TextContent {
            text: text.into(),
            text_signature: None,
        })
    }

    fn thinking_block(text: &str, signature: &str) -> AssistantBlock {
        AssistantBlock::Thinking(crate::ai::types::content::ThinkingContent {
            thinking: text.into(),
            thinking_signature: Some(signature.into()),
            redacted: None,
        })
    }

    fn tool_calls_of(message: &AssistantMessage) -> Vec<&ToolCall> {
        message
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect()
    }

    // ---- 1. minimal text stream: companions, usage, responseId ----

    #[tokio::test]
    async fn minimal_text_stream_events_and_done_message() {
        let server = wiremock::MockServer::start().await;
        mount_sse(&server, sse_body(&minimal_events())).await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "text_end", "done"]
        );
        match &events[0] {
            AssistantMessageEvent::Start { message } => {
                assert_eq!(message.api, "anthropic-messages");
                assert_eq!(message.provider, "anthropic");
                assert_eq!(message.model, "claude-test");
                assert!(message.content.is_empty());
                assert_eq!(message.stop_reason, StopReason::Pending);
                assert_eq!(message.usage, Usage::default());
                assert!(message.timestamp > 0);
            }
            other => panic!("expected Start, got {other:?}"),
        }
        assert_eq!(
            events[2],
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Hello".into()
            }
        );

        let (reason, message) = done_of(&events);
        assert_eq!(*reason, SuccessReason::Stop);
        assert_eq!(message.content, vec![text_block("Hello")]);
        // message_start usage captured, message_delta usage applied.
        assert_eq!(message.usage.input, 12);
        assert_eq!(message.usage.output, 5);
        assert_eq!(message.usage.total_tokens, 17);
        assert_eq!(message.usage.cache_read, 0);
        assert_eq!(message.usage.cache_write, 0);
        // responseId from message_start (responseid oracle behavior).
        assert_eq!(message.response_id.as_deref(), Some("msg_test"));
        assert_eq!(message.response_model, None);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(message.error_message, None);
        assert_eq!(message.stop_reason, StopReason::Stop);

        // The event stream reconstructs through the M2a reducer to exactly the
        // Done message.
        let partial = apply_all(&events);
        assert_eq!(partial.message(), Some(message));
        assert!(partial.is_terminal());
    }

    /// Hostile usage payloads (component values near u64::MAX) saturate the
    /// computed `total_tokens` and the cost basis instead of tripping the
    /// debug overflow check mid-stream.
    #[tokio::test]
    async fn huge_usage_components_saturate_total_tokens() {
        let server = wiremock::MockServer::start().await;
        let huge = u64::MAX - 7;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({
                    "id": "msg_huge",
                    "usage": {"input_tokens": huge, "output_tokens": 0}
                })),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hi"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"input_tokens": huge, "output_tokens": 9, "cache_read_input_tokens": 3, "cache_creation_input_tokens": 4}}),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, message) = done_of(&events);
        assert_eq!(*reason, SuccessReason::Stop);
        // (u64::MAX - 7) + 9 + 3 + 4 saturates instead of overflowing.
        assert_eq!(message.usage.input, huge);
        assert_eq!(message.usage.output, 9);
        assert_eq!(message.usage.total_tokens, u64::MAX);
    }

    // ---- 2. proxy relabels the model: signed thinking stays replayable ----

    #[tokio::test]
    async fn proxy_relabel_keeps_signed_thinking_replayable() {
        // Regression oracle for earendil-works/pi#9188
        // (anthropic-sse-parsing.test.ts:113-138).
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({
                    "id": "msg_response_model",
                    "model": "kimi-for-coding",
                    "usage": {"input_tokens": 100, "output_tokens": 0}
                })),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {
                        "type": "thinking", "thinking": "reasoning", "signature": "signature"
                    }}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"input_tokens": 100, "output_tokens": 20}}),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("Hello")]),
            &StreamOptions::default(),
        )
        .await;

        let (_, message) = done_of(&events);
        assert_eq!(message.model, "claude-test");
        assert_eq!(message.response_model.as_deref(), Some("kimi-for-coding"));
        assert_eq!(
            message.content,
            vec![thinking_block("reasoning", "signature")]
        );
        assert_eq!(message.usage.input, 100);
        assert_eq!(message.usage.output, 20);

        // The signed thinking block replays through the message transformer.
        let transformed = crate::ai::api::openai_completions::request::transform_messages(
            &model,
            &[user_msg("Hello"), Message::Assistant(message.clone())],
            &|id: &str, _source: &crate::ai::types::message::AssistantMessage| id.to_string(),
        );
        let replayed = transformed
            .iter()
            .find(|replayed| matches!(replayed, Message::Assistant(_)))
            .expect("assistant message");
        let Message::Assistant(assistant) = replayed else {
            unreachable!()
        };
        assert_eq!(
            assistant.content,
            vec![thinking_block("reasoning", "signature")]
        );
    }

    // ---- 3. fallback model cost attribution ----

    #[tokio::test]
    async fn fallback_model_used_for_cost_attribution() {
        // anthropic-sse-parsing.test.ts:140-168.
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({
                    "id": "msg_response_model",
                    "model": "fallback-model",
                    "usage": {"input_tokens": 100, "output_tokens": 0}
                })),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "done"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"input_tokens": 100, "output_tokens": 20}}),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({
            "allowedFallbackModels": [
                {"provider": "anthropic", "model": "fallback-model", "cost": {"input": 3, "output": 5, "cacheRead": 0, "cacheWrite": 0}}
            ]
        }));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("Hello")]),
            &StreamOptions::default(),
        )
        .await;

        let (_, message) = done_of(&events);
        assert_eq!(message.model, "claude-test");
        assert_eq!(message.response_model.as_deref(), Some("fallback-model"));
        // 100 tokens at 3/Mtok and 20 at 5/Mtok via the fallback cost.
        assert!((message.usage.cost.input - 0.0003).abs() < 1e-10);
        assert!((message.usage.cost.output - 0.0001).abs() < 1e-10);
    }

    // ---- 4. mid-output fallback fails safely ----

    #[tokio::test]
    async fn mid_output_fallback_fails_safely() {
        // anthropic-sse-parsing.test.ts:170-215.
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({
                    "id": "msg_fallback",
                    "model": "claude-test",
                    "usage": {"input_tokens": 1, "output_tokens": 0}
                })),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "partial"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 1, "content_block": {
                        "type": "fallback",
                        "from": {"model": "claude-test"},
                        "to": {"model": "claude-opus-4-8"}
                    }}),
                ),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("Hello")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, error) = error_of(&events);
        assert_eq!(*reason, ErrorReason::Error);
        assert!(
            error
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("unsupported mid-output model fallback"),
            "{:?}",
            error.error_message
        );
        // The partial text survives the failure through the reducer.
        let partial = apply_all(&events);
        let message = partial.message().expect("error carries the message");
        assert_eq!(message.content, vec![text_block("partial")]);
    }

    // ---- 5. input transformations diagnostic from the final stream event ----

    #[tokio::test]
    async fn input_transformations_from_final_stream_event() {
        // anthropic-sse-parsing.test.ts:288-328.
        let server = wiremock::MockServer::start().await;
        let mut events = minimal_events();
        events[0] = message_start(json!({
            "id": "msg_transformations",
            "model": "claude-test",
            "usage": {"input_tokens": 12, "output_tokens": 0},
            "input_transformations": [
                {"type": "thinking_dropped", "path": "messages.1.content.0", "reason": "prefix_binding_mismatch"}
            ]
        }));
        events[4] = ev(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": delta_usage(),
                "input_transformations": [
                    {"type": "thinking_dropped", "path": "messages.3.content.0", "reason": "model_binding_mismatch"}
                ]
            }),
        );
        mount_sse(&server, sse_body(&events)).await;

        let model = managed_model();
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("Hello")]),
            &StreamOptions::default(),
        )
        .await;

        let (_, message) = done_of(&events);
        let diagnostics = message.diagnostics.as_ref().expect("diagnostics present");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].r#type, "anthropic_input_transformations");
        assert!(diagnostics[0].timestamp > 0);
        assert_eq!(
            diagnostics[0].details,
            Some(json!({
                "transformations": [
                    {"type": "thinking_dropped", "path": "messages.3.content.0", "reason": "model_binding_mismatch"}
                ]
            }))
        );
        // The message_start transformations were replaced, not merged.
        let serialized = serde_json::to_string(&diagnostics).unwrap();
        assert!(
            !serialized.contains("prefix_binding_mismatch"),
            "{serialized}"
        );
    }

    // ---- 6. malformed SSE JSON and malformed streamed tool JSON repaired ----

    #[tokio::test]
    async fn repairs_malformed_sse_json_and_streamed_tool_json() {
        // anthropic-sse-parsing.test.ts:329-414: the event JSON carries an
        // invalid `\H` escape and a raw tab inside the partial_json string, so
        // both the event-level parse and the streamed tool JSON need repair.
        let server = wiremock::MockServer::start().await;
        let malformed = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"A\H\",\"text\":\"col1"#
            .to_string()
            + "\t"
            + r#"col2\"}"}}"#;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({"id": "msg_test", "usage": start_usage()})),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {
                        "type": "tool_use", "id": "toolu_test", "name": "edit", "input": {}
                    }}),
                ),
                raw_ev("content_block_delta", &malformed),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": delta_usage()}),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &tools_ctx(vec![user_msg("Use the edit tool.")], vec![edit_tool()]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, message) = done_of(&events);
        assert_eq!(*reason, SuccessReason::ToolUse);
        assert_eq!(message.error_message, None);
        let calls = tool_calls_of(message);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_test");
        assert_eq!(calls[0].name, "edit");
        assert_eq!(
            calls[0].arguments,
            json!({"path": "A\\H", "text": "col1\tcol2"})
        );
    }

    // ---- 7. content from content_block_start preserved ----

    #[tokio::test]
    async fn preserves_content_from_content_block_start() {
        // anthropic-sse-parsing.test.ts:416-512.
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({
                    "id": "msg_initial_content",
                    "usage": start_usage()
                })),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "Initial text"}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " plus delta"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 1, "content_block": {
                        "type": "thinking", "thinking": "Initial thinking", "signature": "initial signature"
                    }}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 1, "delta": {"type": "thinking_delta", "thinking": " plus delta"}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 1, "delta": {"type": "signature_delta", "signature": " plus delta"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 1}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": delta_usage()}),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("Say hello.")]),
            &StreamOptions::default(),
        )
        .await;

        assert_eq!(
            event_types(&events),
            [
                "start",
                "text_start",
                "text_delta",
                "text_end",
                "thinking_start",
                "thinking_delta",
                "thinking_end",
                "done"
            ]
        );
        let (_, message) = done_of(&events);
        assert_eq!(
            message.content,
            vec![
                text_block("Initial text plus delta"),
                thinking_block(
                    "Initial thinking plus delta",
                    "initial signature plus delta"
                ),
            ]
        );
    }

    // ---- 8. refusal stop details ----

    #[tokio::test]
    async fn preserves_refusal_stop_details() {
        // anthropic-sse-parsing.test.ts:514-571.
        let server = wiremock::MockServer::start().await;
        let explanation = "This request triggered restrictions on violative cyber content and was blocked under Anthropic's Usage Policy.";
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({
                    "id": "msg_01XFUDYJgAACzvnptvVoYEL",
                    "usage": {"input_tokens": 412, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}
                })),
                ev(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {
                            "stop_reason": "refusal",
                            "stop_details": {"type": "refusal", "category": "cyber", "explanation": explanation}
                        },
                        "usage": {"input_tokens": 412, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}
                    }),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("blocked request")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, error) = error_of(&events);
        assert_eq!(*reason, ErrorReason::Error);
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(error.raw_stop_reason.as_deref(), Some("refusal"));
        assert_eq!(error.error_message.as_deref(), Some(explanation));
        assert!(apply_all(&events).is_terminal());
    }

    // ---- 9. sensitive stop reason ----

    #[tokio::test]
    async fn preserves_sensitive_stop_reason() {
        // anthropic-sse-parsing.test.ts:573-621.
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({"id": "msg_sensitive", "usage": start_usage()})),
                ev(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": "sensitive"},
                        "usage": {"input_tokens": 12, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}
                    }),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("blocked request")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, error) = error_of(&events);
        assert_eq!(*reason, ErrorReason::Error);
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(error.raw_stop_reason.as_deref(), Some("sensitive"));
        assert_eq!(
            error.error_message.as_deref(),
            Some("Provider stopped with: sensitive")
        );
    }

    // ---- 10. message_delta without usage is a no-op for accumulation ----

    #[tokio::test]
    async fn message_delta_without_usage_is_a_noop() {
        // anthropic-sse-parsing.test.ts:623-649.
        let server = wiremock::MockServer::start().await;
        let events = minimal_events()
            .iter()
            .map(|event| {
                if event.starts_with("event: message_delta") {
                    ev(
                        "message_delta",
                        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
                    )
                } else {
                    event.clone()
                }
            })
            .collect::<Vec<_>>();
        mount_sse(&server, sse_body(&events)).await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("Say hello.")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, message) = done_of(&events);
        assert_eq!(*reason, SuccessReason::Stop);
        assert_eq!(message.error_message, None);
        assert_eq!(message.content, vec![text_block("Hello")]);
        // The message_start usage survives the usage-less message_delta.
        assert_eq!(message.usage.input, 12);
        assert_eq!(message.usage.total_tokens, 12);
    }

    // ---- 11. unknown SSE events ignored ----

    #[tokio::test]
    async fn ignores_unknown_sse_events_after_message_stop() {
        // anthropic-sse-parsing.test.ts:651-670.
        let server = wiremock::MockServer::start().await;
        let mut events = minimal_events();
        events.push(raw_ev("done", "[DONE]"));
        events.push(raw_ev("proxy.stats", "not json"));
        mount_sse(&server, sse_body(&events)).await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("Say hello.")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, message) = done_of(&events);
        assert_eq!(*reason, SuccessReason::Stop);
        assert_eq!(message.error_message, None);
        assert_eq!(message.content, vec![text_block("Hello")]);
    }

    // ---- 12/13. cache write 1h cost split ----

    #[tokio::test]
    async fn cache_write_1h_portion_priced_at_twice_the_input_rate() {
        // anthropic-cache-write-1h-cost.test.ts:62-75.
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({
                    "id": "msg_test",
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "cache_creation_input_tokens": 1_000_000,
                        "cache_creation": {"ephemeral_5m_input_tokens": 600_000, "ephemeral_1h_input_tokens": 400_000}
                    }
                })),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hi"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": "end_turn"},
                        "usage": {
                            "input_tokens": 100,
                            "output_tokens": 5,
                            "cache_read_input_tokens": 0,
                            "cache_creation_input_tokens": 1_000_000
                        }
                    }),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = opus_priced_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        let (_, message) = done_of(&events);
        assert_eq!(message.usage.cache_write, 1_000_000);
        assert_eq!(message.usage.cache_write_1h, Some(400_000));
        // 600k * 6.25/Mtok + 400k * 10/Mtok = 3.75 + 4.0 = 7.75
        assert!((message.usage.cost.cache_write - 7.75).abs() < 1e-10);
    }

    #[tokio::test]
    async fn cache_write_falls_back_to_5m_rate_without_breakdown() {
        // anthropic-cache-write-1h-cost.test.ts:77-88.
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({
                    "id": "msg_test",
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "cache_creation_input_tokens": 1_000_000
                    }
                })),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hi"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": "end_turn"},
                        "usage": {
                            "input_tokens": 100,
                            "output_tokens": 5,
                            "cache_read_input_tokens": 0,
                            "cache_creation_input_tokens": 1_000_000
                        }
                    }),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = opus_priced_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        let (_, message) = done_of(&events);
        assert_eq!(message.usage.cache_write, 1_000_000);
        // Upstream `|| 0`: the field is always a number after message_start.
        assert_eq!(message.usage.cache_write_1h, Some(0));
        // 1M * 6.25/Mtok = 6.25
        assert!((message.usage.cost.cache_write - 6.25).abs() < 1e-10);
    }

    // ---- 14. multi tool_use accumulators (M2a fix retained) ----

    #[tokio::test]
    async fn parallel_tool_use_blocks_keep_separate_accumulators() {
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({"id": "msg_test", "usage": start_usage()})),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "running both"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "t1", "name": "read"}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"pa"}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "th\": \"a.txt\"}"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 1}),
                ),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "t2", "name": "bash"}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"comm"}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "and\": \"ls\"}"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 2}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": delta_usage()}),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, message) = done_of(&events);
        assert_eq!(*reason, SuccessReason::ToolUse);
        let calls = tool_calls_of(message);
        assert_eq!(calls.len(), 2, "got: {calls:?}");
        assert_eq!(calls[0].id, "t1");
        assert_eq!(calls[0].arguments, json!({"path": "a.txt"}));
        assert_eq!(calls[1].id, "t2");
        assert_eq!(calls[1].arguments, json!({"command": "ls"}));
        let partial = apply_all(&events);
        assert_eq!(partial.message(), Some(message));
    }

    // ---- 15. stop reason mapping ----

    #[tokio::test]
    async fn stop_reason_mapping() {
        let cases: [(&str, SuccessReason, StopReason); 5] = [
            ("end_turn", SuccessReason::Stop, StopReason::Stop),
            ("max_tokens", SuccessReason::Length, StopReason::Length),
            ("tool_use", SuccessReason::ToolUse, StopReason::ToolUse),
            // "Stop is good enough -> resubmit" (upstream line 1510).
            ("pause_turn", SuccessReason::Stop, StopReason::Stop),
            ("stop_sequence", SuccessReason::Stop, StopReason::Stop),
        ];
        for (stop_reason, expected_reason, expected_stop) in cases {
            let server = wiremock::MockServer::start().await;
            let mut events = minimal_events();
            events[4] = ev(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": stop_reason},
                    "usage": delta_usage()
                }),
            );
            mount_sse(&server, sse_body(&events)).await;

            let model = make_model(json!({}));
            let events = collect_stream(
                &server,
                &model,
                &user_ctx(vec![user_msg("hi")]),
                &StreamOptions::default(),
            )
            .await;

            let (reason, message) = done_of(&events);
            assert_eq!(*reason, expected_reason, "stop_reason {stop_reason}");
            assert_eq!(
                message.stop_reason, expected_stop,
                "stop_reason {stop_reason}"
            );
            assert_eq!(
                message.raw_stop_reason.as_deref(),
                Some(stop_reason),
                "raw stop reason preserved"
            );
            assert_eq!(message.error_message, None);
        }
    }

    // ---- 16. unhandled stop reasons fail with the raw reason preserved ----

    #[tokio::test]
    async fn unhandled_stop_reason_errors_with_raw_preserved() {
        let server = wiremock::MockServer::start().await;
        let mut events = minimal_events();
        events[4] = ev(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "model_context_window"},
                "usage": delta_usage()
            }),
        );
        mount_sse(&server, sse_body(&events)).await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, error) = error_of(&events);
        assert_eq!(*reason, ErrorReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Unhandled stop reason: model_context_window")
        );
        assert_eq!(
            error.raw_stop_reason.as_deref(),
            Some("model_context_window")
        );
        // Partial content is kept.
        let partial = apply_all(&events);
        let message = partial.message().expect("message");
        assert_eq!(message.content, vec![text_block("Hello")]);
    }

    // ---- 17. SSE error events use the raw data payload as the message ----

    #[tokio::test]
    async fn sse_error_event_uses_raw_data_as_message() {
        let server = wiremock::MockServer::start().await;
        let error_data =
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let mut events = minimal_events();
        events.truncate(4); // through content_block_stop
        events.push(raw_ev("error", error_data));
        mount_sse(&server, sse_body(&events)).await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, error) = error_of(&events);
        assert_eq!(*reason, ErrorReason::Error);
        // Upstream `throw new Error(sse.data)`: the raw payload, not a parsed
        // field of it.
        assert_eq!(error.error_message.as_deref(), Some(error_data));
        // Mid-stream failure keeps the partial content.
        let partial = apply_all(&events);
        let message = partial.message().expect("message");
        assert_eq!(message.content, vec![text_block("Hello")]);
    }

    // ---- 18. stream ends before message_stop ----

    #[tokio::test]
    async fn stream_ended_before_message_stop_errors() {
        let server = wiremock::MockServer::start().await;
        let mut events = minimal_events();
        events.truncate(4); // no message_delta / message_stop
        mount_sse(&server, sse_body(&events)).await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, error) = error_of(&events);
        assert_eq!(*reason, ErrorReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Anthropic stream ended before message_stop")
        );
    }

    // ---- 19. stream without a stop reason ----

    #[tokio::test]
    async fn stream_without_stop_reason_errors() {
        let server = wiremock::MockServer::start().await;
        let mut events = minimal_events();
        // message_delta with usage only, no stop_reason; message_stop present.
        events[4] = ev(
            "message_delta",
            json!({"type": "message_delta", "delta": {}, "usage": delta_usage()}),
        );
        mount_sse(&server, sse_body(&events)).await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        let (reason, error) = error_of(&events);
        assert_eq!(*reason, ErrorReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Anthropic stream ended without a stop reason")
        );
    }

    // ---- 20. redacted thinking ----

    #[tokio::test]
    async fn redacted_thinking_block() {
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({"id": "msg_test", "usage": start_usage()})),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "redacted_thinking", "data": "ENCRYPTED"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": delta_usage()}),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        assert_eq!(
            event_types(&events),
            ["start", "thinking_start", "thinking_end", "done"]
        );
        let (_, message) = done_of(&events);
        assert_eq!(
            message.content,
            vec![AssistantBlock::Thinking(
                crate::ai::types::content::ThinkingContent {
                    thinking: "[Reasoning redacted]".into(),
                    thinking_signature: Some("ENCRYPTED".into()),
                    redacted: Some(true),
                }
            )]
        );
        let partial = apply_all(&events);
        assert_eq!(partial.message(), Some(message));
    }

    // ---- 21. reasoning tokens from message_delta ----

    #[tokio::test]
    async fn reasoning_tokens_from_message_delta() {
        let server = wiremock::MockServer::start().await;
        let mut events = minimal_events();
        events[4] = ev(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"input_tokens": 12, "output_tokens": 5, "output_tokens_details": {"thinking_tokens": 3}}
            }),
        );
        mount_sse(&server, sse_body(&events)).await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        let (_, message) = done_of(&events);
        assert_eq!(message.usage.output, 5);
        assert_eq!(message.usage.reasoning, Some(3));
        assert_eq!(message.usage.total_tokens, 17);
    }

    // ---- 22. pre-start failures are lone Error events ----

    #[tokio::test]
    async fn http_error_before_start_is_a_lone_error_event() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;

        let model = make_model(json!({}));
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        assert_eq!(events.len(), 1, "{events:?}");
        let (reason, error) = error_of(&events);
        assert_eq!(*reason, ErrorReason::Error);
        assert!(
            error
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("401"),
            "{:?}",
            error.error_message
        );
        assert_eq!(error.api, "anthropic-messages");
        // Reducer-compatible: a pre-start error is allowed and terminal.
        let partial = apply_all(&events);
        assert!(partial.message().is_some());
        assert!(partial.is_terminal());
    }

    #[tokio::test]
    async fn missing_api_key_is_a_lone_error_event() {
        let server = wiremock::MockServer::start().await;
        mount_sse(&server, sse_body(&minimal_events())).await;

        let model = make_model(json!({}));
        let api = AnthropicMessages;
        let mut rx = api.stream(
            &ProviderConfig {
                base_url: server.uri(),
                api_key: String::new(),
                max_tokens: 32000,
            },
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        );
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        assert_eq!(events.len(), 1, "{events:?}");
        let (reason, error) = error_of(&events);
        assert_eq!(*reason, ErrorReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("No API key for provider: anthropic")
        );
    }

    // ---- 23. managed-effort models stamp providerThinkingLevel ----

    #[tokio::test]
    async fn managed_effort_stamps_provider_thinking_level() {
        let server = wiremock::MockServer::start().await;
        mount_sse(&server, sse_body(&minimal_events())).await;

        // stream() without an explicit effort: the default "high".
        let model = managed_model();
        let events = collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;
        match &events[0] {
            AssistantMessageEvent::Start { message } => {
                assert_eq!(message.provider_thinking_level.as_deref(), Some("high"));
            }
            other => panic!("expected Start, got {other:?}"),
        }
        let (_, message) = done_of(&events);
        assert_eq!(message.provider_thinking_level.as_deref(), Some("high"));
    }

    // ---- 24. stream vs streamSimple request shaping ----

    #[tokio::test]
    async fn stream_vs_stream_simple_thinking_request_shaping() {
        let server = wiremock::MockServer::start().await;
        mount_sse(&server, sse_body(&minimal_events())).await;

        // stream(): base options only — no thinking param at all.
        let model = make_model(json!({}));
        collect_stream(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &StreamOptions::default(),
        )
        .await;

        // streamSimple() with a reasoning level: budget-based thinking.
        let simple = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Medium),
            ..SimpleStreamOptions::default()
        };
        let events =
            collect_simple(&server, &model, &user_ctx(vec![user_msg("hi")]), &simple).await;
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));

        // streamSimple() without reasoning: thinking explicitly disabled.
        let simple = SimpleStreamOptions::default();
        let events =
            collect_simple(&server, &model, &user_ctx(vec![user_msg("hi")]), &simple).await;
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 3);
        for request in &requests {
            assert_eq!(request.url.path(), "/v1/messages");
            assert_eq!(
                request
                    .headers
                    .get("x-api-key")
                    .map(|value| value.to_str().unwrap()),
                Some("test-key")
            );
            assert_eq!(
                request
                    .headers
                    .get("anthropic-version")
                    .map(|value| value.to_str().unwrap()),
                Some("2023-06-01")
            );
        }
        let bodies: Vec<Value> = requests
            .iter()
            .map(|request| serde_json::from_slice(&request.body).unwrap())
            .collect();
        assert!(bodies[0].get("thinking").is_none(), "{}", bodies[0]);
        assert_eq!(
            bodies[1]["thinking"],
            json!({"type": "enabled", "budget_tokens": 8192, "display": "summarized"})
        );
        assert_eq!(bodies[2]["thinking"], json!({"type": "disabled"}));
    }

    // ---- 24b. OAuth tool_use names map back to the declared tools ----

    #[tokio::test]
    async fn oauth_tool_use_names_map_back_to_declared_tools() {
        // Upstream lines 664-666: with OAuth, a content_block_start tool_use
        // name goes through fromClaudeCodeName — the Claude Code canonical
        // casing maps back to the declared tool's name.
        let server = wiremock::MockServer::start().await;
        mount_sse(
            &server,
            sse_body(&[
                message_start(json!({"id": "msg_test", "usage": start_usage()})),
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t1", "name": "TodoWrite"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": delta_usage()}),
                ),
                ev("message_stop", json!({"type": "message_stop"})),
            ]),
        )
        .await;

        let model = make_model(json!({}));
        let api = AnthropicMessages;
        let mut rx = api.stream(
            &cfg(&server),
            &model,
            &tools_ctx(vec![user_msg("hi")], vec![tool_decl("todowrite")]),
            &StreamOptions {
                api_key: Some("sk-ant-oat01-test".to_string()),
                ..StreamOptions::default()
            },
        );
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        let (_, message) = done_of(&events);
        let calls = tool_calls_of(message);
        assert_eq!(calls.len(), 1);
        // "TodoWrite" maps back to the declared "todowrite".
        assert_eq!(calls[0].name, "todowrite");

        // Without OAuth the wire name passes through unchanged.
        mount_sse(&server, sse_body(&minimal_events())).await;
        let mut rx = api.stream(
            &cfg(&server),
            &model,
            &tools_ctx(vec![user_msg("hi")], vec![tool_decl("todowrite")]),
            &StreamOptions::default(),
        );
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    // ---- 25. streamSimple adaptive thinking and budgets ----

    #[tokio::test]
    async fn stream_simple_thinking_budgets_and_adaptive() {
        let server = wiremock::MockServer::start().await;
        mount_sse(&server, sse_body(&minimal_events())).await;

        // Adaptive models map the level to an effort.
        let model = managed_model();
        let simple = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Medium),
            ..SimpleStreamOptions::default()
        };
        let options = options_from_simple(&model, &user_ctx(vec![user_msg("hi")]), &simple);
        assert_eq!(
            options.effort,
            Some(crate::ai::api::anthropic::request::AnthropicEffort::Medium)
        );
        let events =
            collect_simple(&server, &model, &user_ctx(vec![user_msg("hi")]), &simple).await;
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));

        // Custom thinking budgets flow into budget_tokens (legacy model).
        let model = make_model(json!({}));
        let simple = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Low),
            thinking_budgets: Some(crate::ai::types::primitives::ThinkingBudgets {
                low: Some(2048),
                ..crate::ai::types::primitives::ThinkingBudgets::default()
            }),
            ..SimpleStreamOptions::default()
        };
        let events =
            collect_simple(&server, &model, &user_ctx(vec![user_msg("hi")]), &simple).await;
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));

        let requests = server.received_requests().await.unwrap();
        let bodies: Vec<Value> = requests
            .iter()
            .map(|request| serde_json::from_slice(&request.body).unwrap())
            .collect();
        // Upstream buildParams hardcodes the managed-model top-level
        // output_config to "high" (line 1159); the mapped effort rides on the
        // trailing thinking-level marker (activeEffort).
        assert_eq!(
            bodies[0]["messages"].as_array().unwrap().last().unwrap()["output_config"],
            json!({"effort": "medium"}),
            "{}",
            bodies[0]
        );
        assert_eq!(bodies[1]["thinking"]["budget_tokens"], json!(2048));
    }

    // ---- 26. max_tokens/metadata options reach the wire ----

    #[tokio::test]
    async fn max_tokens_option_reaches_the_wire() {
        let server = wiremock::MockServer::start().await;
        mount_sse(&server, sse_body(&minimal_events())).await;

        let model = make_model(json!({}));
        let options = StreamOptions {
            max_tokens: Some(1024),
            metadata: Some(BTreeMap::from([("user_id".to_string(), json!("user-1"))])),
            ..StreamOptions::default()
        };
        let events =
            collect_stream(&server, &model, &user_ctx(vec![user_msg("hi")]), &options).await;
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));

        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["max_tokens"], json!(1024));
        assert_eq!(body["metadata"], json!({"user_id": "user-1"}));
        assert_eq!(body["stream"], json!(true));
    }

    // ---- T8: provider-request retry on the send seam ----

    /// First POST answers 500 (with a tiny server-requested delay), later
    /// POSTs stream a complete message.
    struct RetryFlaky {
        attempts: std::sync::atomic::AtomicU32,
        success: wiremock::ResponseTemplate,
    }

    impl wiremock::Respond for RetryFlaky {
        fn respond(&self, _request: &wiremock::Request) -> wiremock::ResponseTemplate {
            if self
                .attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 0
            {
                wiremock::ResponseTemplate::new(500)
                    .insert_header("retry-after-ms", "15")
                    .set_body_string("overloaded")
            } else {
                self.success.clone()
            }
        }
    }

    #[tokio::test]
    async fn send_seam_retries_a_500_before_the_first_stream_byte() {
        let server = wiremock::MockServer::start().await;
        let body = sse_body(&[
            message_start(json!({"id": "msg_retry", "usage": start_usage()})),
            ev(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            ),
            ev(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            ),
            ev(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": delta_usage()}),
            ),
            ev("message_stop", json!({"type": "message_stop"})),
        ]);
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(RetryFlaky {
                attempts: std::sync::atomic::AtomicU32::new(0),
                success: sse(&body),
            })
            .mount(&server)
            .await;

        let model = make_model(json!({}));
        let options = StreamOptions {
            max_retries: Some(1),
            ..Default::default()
        };
        let events =
            collect_stream(&server, &model, &user_ctx(vec![user_msg("hi")]), &options).await;
        assert!(
            matches!(events.last(), Some(AssistantMessageEvent::Done { .. })),
            "{events:?}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn send_seam_does_not_retry_a_401() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;

        let model = make_model(json!({}));
        let options = StreamOptions {
            max_retries: Some(3),
            ..Default::default()
        };
        let events =
            collect_stream(&server, &model, &user_ctx(vec![user_msg("hi")]), &options).await;
        assert_eq!(events.len(), 1, "lone error event: {events:?}");
        assert!(matches!(events[0], AssistantMessageEvent::Error { .. }));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod callback_tests;
