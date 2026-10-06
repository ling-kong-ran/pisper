//! OpenAI-completions streaming — full port of the stream/streamSimple
//! implementations from upstream `packages/ai/src/api/openai-completions.ts`
//! (lines 299-749): the SSE chunk loop (lines 553-677), block accumulation
//! with canonical `text_start`/`thinking_start`/`toolcall_start` companions,
//! the end-of-stream finish pass (lines 679-699), usage parsing
//! (`parseChunkUsage`, lines 1509-1550) with `calculateCost`, finish-reason
//! mapping (`mapStopReason`, lines 1552-1576) with raw-stop-reason
//! preservation, `responseModel`/`responseId` capture, and pre-start vs
//! mid-stream error handling (catch block, lines 701-724).
//!
//! Deviations from upstream, all structural:
//! - Upstream events carry the live `partial`; the port emits events without
//!   it and consumers reconstruct via `PartialAssistant` (the M2a contract).
//! - Upstream `options.signal` is the port's non-serialized
//!   [`CancellationToken`](tokio_util::sync::CancellationToken): a
//!   pre-cancelled token or cancellation during the initial request fails
//!   before `Start` with the retry seam's `"Request aborted"` abort error and
//!   is never retried; a cancellation after `Start` breaks the chunk read and
//!   the catch block settles `stopReason: "aborted"` with
//!   `"Request was aborted"` (upstream lines 682-688).
//! - `parseStreamingJson` (`utils/json-parse.ts`) falls back to the
//!   `partial-json` package; no equivalent crate is a dependency, so the
//!   partial-parse step is approximated by closing open strings/brackets and
//!   completing dangling values (see [`parse_streaming_json`]). The never-throw
//!   `{}` fallback contract is preserved.
//! - Process-local `onPayload`/`onResponse` are awaited around the actual
//!   HTTP request and before Start/body consumption. The send seam is [`send_stream_request`], which applies the T8 provider-request retry
//!   port ([`crate::ai::retry::retry_provider_request`]) to the initial HTTP
//!   request; mid-flight stream errors are never retried.
//! - HTTP error bodies are surfaced as `"{status}: {body}"` (the upstream
//!   `formatProviderError` composition with the parsed body); a non-JSON body
//!   is used verbatim where the openai SDK keeps only its own message.
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order — same documented deviation as the request builder.

use std::collections::HashMap;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::ai::api::openai_completions::compat_detect::{
    detect_openai_completions_compat_for_model, merge_compat,
};
use crate::ai::api::openai_completions::request::{
    build_request, create_grammar_tool_input_properties, is_openai_reasoning_detail,
    RequestAssembly,
};
use crate::ai::api::{http_client, request_signal, ApiImpl, REQUEST_WAS_ABORTED};
use crate::ai::cost::calculate_cost;
use crate::ai::json_parse::repair_json;
use crate::ai::retry::{retry_provider_request, ProviderError};
use crate::ai::transcript::{get_declared_tools, resolve_transcript, TranscriptContext};
use crate::ai::types::compat::OpenAiCompletionsCompat;
use crate::ai::types::content::{ThinkingContent, ToolCall};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::message::{AssistantBlock, AssistantMessage};
use crate::ai::types::options::{ProviderHeaders, SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{StopReason, Usage, UsageCost};
use crate::ai::types::Model;
use crate::ai::{now_ms, ProviderConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The API id stamped on every emitted message.
const API: &str = "openai-completions";

/// Upstream `MAX_PROVIDER_ERROR_BODY_CHARS` (`utils/error-body.ts`); shared
/// with the azure-openai-responses error composer.
pub(crate) const MAX_PROVIDER_ERROR_BODY_CHARS: usize = 4000;

pub struct OpenAiCompletions;

impl ApiImpl for OpenAiCompletions {
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
        // Upstream `stream` takes the API-specific option extension
        // (`toolChoice`/`reasoningEffort`/`thinkingBudgets`) on top of the
        // base options; the port's `StreamOptions` is the base set, so the
        // extension fields stay unset for direct `stream` calls (the provider
        // defaults from `compat`/`model` apply, exactly like an upstream
        // caller passing none of them).
        let simple = SimpleStreamOptions {
            stream: options.clone(),
            ..SimpleStreamOptions::default()
        };
        run_stream(cfg.clone(), model.clone(), ctx.clone(), simple)
    }

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        run_stream(cfg.clone(), model.clone(), ctx.clone(), options.clone())
    }
}

fn run_stream(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: SimpleStreamOptions,
) -> mpsc::Receiver<AssistantMessageEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        run_stream_task(cfg, model, ctx, options, tx).await;
    });
    rx
}

/// Accumulation state for one stream, the port of the closures/locals over the
/// upstream loop (`textBlock`, `thinkingBlock`, `toolCallBlocksByIndex/Id`,
/// `streamedReasoningDetails`, ...). Content indexes are positions in
/// `output.content`; blocks are appended in event order and never removed.
struct StreamState {
    output: AssistantMessage,
    text_index: Option<usize>,
    thinking_index: Option<usize>,
    /// Wire `tool_calls[].index` -> content index.
    tool_by_wire: HashMap<u64, usize>,
    /// Tool call id -> content index.
    tool_by_id: HashMap<String, usize>,
    /// Per-tool-call scratch (upstream `partialArgs`/`customInput`/
    /// `streamIndex`), keyed by content index.
    tool_accs: HashMap<usize, ToolAcc>,
    /// Upstream `grammarToolInputProperties`: tool name -> custom input
    /// property for grammar-constrained tools.
    grammar_props: HashMap<String, String>,
    /// Upstream `streamedReasoningDetails`: replay metadata serialized into
    /// the thinking signature when the block finalizes.
    streamed_reasoning_details: Option<Vec<Value>>,
    has_finish_reason: bool,
}

/// Upstream `StreamingToolCallBlock` scratch fields.
struct ToolAcc {
    stream_index: Option<u64>,
    /// Upstream `partialArgs`: raw `function.arguments` fragments.
    partial_args: String,
    /// Upstream `customInput` (grammar/custom tool calls).
    custom_input: Option<CustomInput>,
}

struct CustomInput {
    property: String,
    buf: GrammarBuf,
}

/// Upstream `GrammarToolInputJsonBuffer`
/// (`api/constrained-sampling.ts:139-144`). Shared with the openai-responses
/// port (custom tool-call input streaming).
#[derive(Debug, Default, Clone)]
pub(crate) struct GrammarBuf {
    pub(crate) input: String,
    started: bool,
    closed: bool,
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
                usage: Usage::default(),
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: now_ms(),
            },
            text_index: None,
            thinking_index: None,
            tool_by_wire: HashMap::new(),
            tool_by_id: HashMap::new(),
            tool_accs: HashMap::new(),
            grammar_props: HashMap::new(),
            streamed_reasoning_details: None,
            has_finish_reason: false,
        }
    }
}

/// Upstream `getCompat`: detection + explicit `model.compat` overrides.
fn get_compat(model: &Model) -> OpenAiCompletionsCompat {
    merge_compat(
        detect_openai_completions_compat_for_model(&model.provider, &model.base_url, &model.id),
        model.compat.as_ref(),
    )
}

/// Upstream `getClientApiKey` (lines 82-86): the effective bearer key, or
/// `"unused"` when the caller already supplies gateway auth headers.
pub(crate) fn get_client_api_key(
    provider: &str,
    api_key: &str,
    headers: Option<&ProviderHeaders>,
) -> Result<String, String> {
    if !api_key.is_empty() {
        return Ok(api_key.to_string());
    }
    if has_header(headers, "authorization") || has_header(headers, "cf-aig-authorization") {
        return Ok("unused".to_string());
    }
    Err(format!("No API key for provider: {provider}"))
}

/// Upstream `hasHeader` (lines 73-80): case-insensitive presence with a
/// non-empty value.
fn has_header(headers: Option<&ProviderHeaders>, name: &str) -> bool {
    headers.into_iter().flatten().any(|(key, value)| {
        key.eq_ignore_ascii_case(name)
            && value
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    })
}

/// Upstream `truncateErrorText` (`utils/error-body.ts`), char-count based;
/// shared with the azure-openai-responses error composer.
pub(crate) fn truncate_error_text(text: &str, max_chars: usize) -> String {
    let length = text.chars().count();
    if length <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}... [truncated {} chars]", length - max_chars)
}

/// Error message for a non-2xx response: upstream's `formatProviderError`
/// composition for the openai SDK error shape — `"{status}: {body}"` with the
/// JSON-stringified parsed body, plus the OpenRouter `error.metadata.raw`
/// append from the catch block (lines 714-721). Shared with the
/// anthropic-messages port.
pub(crate) fn format_http_error(status: u16, body_text: &str) -> String {
    let trimmed = body_text.trim();
    let (message, raw_metadata) = if trimmed.is_empty() {
        (format!("{status} status code with empty body"), None)
    } else {
        match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => {
                let raw = value
                    .pointer("/error/metadata/raw")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                (
                    format!(
                        "{status}: {}",
                        truncate_error_text(&value.to_string(), MAX_PROVIDER_ERROR_BODY_CHARS)
                    ),
                    raw,
                )
            }
            Err(_) => (
                format!(
                    "{status}: {}",
                    truncate_error_text(trimmed, MAX_PROVIDER_ERROR_BODY_CHARS)
                ),
                None,
            ),
        }
    };
    match raw_metadata {
        Some(raw) if !message.contains(&raw) => format!("{message}\n{raw}"),
        _ => message,
    }
}

/// Send the assembled request (upstream `client.chat.completions.create`),
/// wrapped in the provider retry policy with `options.maxRetries` /
/// `maxRetryDelayMs` (upstream invokes the SDK with `maxRetries: 0` and wraps
/// `retryProviderRequest` around the call). Retries cover transport failures
/// and retryable statuses only — the SDK throws before the response stream is
/// returned, so once stream bytes flow an error is never retried.
async fn send_stream_request(
    cfg: &ProviderConfig,
    api_key: &str,
    assembly: &RequestAssembly,
    options: &SimpleStreamOptions,
    signal: &CancellationToken,
) -> Result<reqwest::Response, String> {
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    // Bearer auth first: assembly headers (model.headers then the caller's
    // options headers) are inserted after and override it — the upstream
    // SDK lets `defaultHeaders` override the SDK auth header, which is what
    // makes the gateway `"unused"` key flow in `getClientApiKey` work.
    let mut headers = reqwest::header::HeaderMap::new();
    let auth = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|error| format!("Invalid authorization header: {error}"))?;
    headers.insert(reqwest::header::AUTHORIZATION, auth);
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
    options: SimpleStreamOptions,
    tx: mpsc::Sender<AssistantMessageEvent>,
) {
    let mut state = StreamState::new(&model);
    let signal = request_signal(&options.stream.signal);
    match drive_stream(&cfg, &model, &ctx, &options, &signal, &mut state, &tx).await {
        Ok(()) => {}
        Err(message) => {
            // Upstream catch block (lines 701-724): thinking signatures get
            // the streamed reasoning details; stopReason settles to "aborted"
            // when the request signal fired, else "error"; the thrown value
            // becomes the errorMessage carried by the error event.
            if let Some(details) = &state.streamed_reasoning_details {
                if let Ok(signature) = serde_json::to_string(details) {
                    for block in &mut state.output.content {
                        if let AssistantBlock::Thinking(thinking) = block {
                            thinking.thinking_signature = Some(signature.clone());
                        }
                    }
                }
            }
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
    options: &SimpleStreamOptions,
    signal: &CancellationToken,
    state: &mut StreamState,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let compat = get_compat(model);
    let api_key = get_client_api_key(
        &model.provider,
        &cfg.api_key,
        options.stream.headers.as_ref(),
    )?;
    // Upstream resolves the transcript once at the top of `stream` (line 305).
    let normalized = resolve_transcript(ctx.clone(), compat.supports_mid_convo_system_messages);
    // Grammar input properties before request assembly, matching upstream's
    // ordering (lines 338-341 before line 353) for error precedence.
    state.grammar_props = create_grammar_tool_input_properties(
        &get_declared_tools(normalized.messages()),
        compat.supports_openai_grammar_tools == Some(true),
    )?;
    let mut assembly = build_request(model, cfg, &normalized, options, &compat)?;
    assembly.body = options
        .stream
        .callbacks
        .payload(assembly.body, model)
        .await
        .map_err(|e| e.to_string())?;

    let response = send_stream_request(cfg, &api_key, &assembly, options, signal).await?;
    options
        .stream
        .callbacks
        .http_response(&response, model)
        .await
        .map_err(|e| e.to_string())?;

    // Upstream line 379: `start` after the response arrives, before any chunk.
    let _ = tx
        .send(AssistantMessageEvent::Start {
            message: state.output.clone(),
        })
        .await;

    let mut events = response.bytes_stream().eventsource();
    // Upstream line 682 checks `signal?.aborted` around the chunk loop; the
    // select breaks the read the moment the token cancels.
    loop {
        let item = tokio::select! {
            biased;
            _ = signal.cancelled() => return Err(REQUEST_WAS_ABORTED.to_string()),
            next = events.next() => match next {
                Some(item) => item,
                None => break,
            },
        };
        let event = item.map_err(|error| error.to_string())?;
        if event.data.trim() == "[DONE]" {
            break;
        }
        let chunk: Value = serde_json::from_str(&event.data)
            .map_err(|error| format!("Could not parse chunk: {error}"))?;
        process_chunk(state, &chunk, model, tx).await?;
    }

    // Upstream lines 679-681: finish every block in content order.
    for content_index in 0..state.output.content.len() {
        finish_block(state, content_index, tx).await?;
    }

    // Upstream line 682: the post-loop abort check precedes every other
    // terminal guard.
    if signal.is_cancelled() {
        return Err(REQUEST_WAS_ABORTED.to_string());
    }

    // Upstream lines 689-697: stop-reason inference and terminal checks.
    let supports_finish_reason = compat.supports_finish_reason != Some(false);
    if !state.has_finish_reason && !supports_finish_reason {
        state.output.stop_reason = if state
            .output
            .content
            .iter()
            .any(|block| matches!(block, AssistantBlock::ToolCall(_)))
        {
            StopReason::ToolUse
        } else {
            StopReason::Stop
        };
    }
    if state.output.stop_reason == StopReason::Error {
        return Err(state
            .output
            .error_message
            .clone()
            .unwrap_or_else(|| "Provider returned an error stop reason".to_string()));
    }
    if (supports_finish_reason && !state.has_finish_reason)
        || state.output.stop_reason == StopReason::Pending
    {
        return Err("Stream ended without finish_reason".to_string());
    }

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

/// One SSE `data:` chunk (upstream lines 554-677).
async fn process_chunk(
    state: &mut StreamState,
    chunk: &Value,
    model: &Model,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    // Upstream line 554: non-object chunks are skipped.
    if !chunk.is_object() {
        return Ok(());
    }

    // Upstream lines 556-561: response id and routed response model.
    if state.output.response_id.is_none() {
        if let Some(id) = chunk.get("id").and_then(Value::as_str) {
            state.output.response_id = Some(id.to_string());
        }
    }
    if let Some(chunk_model) = chunk.get("model").and_then(Value::as_str) {
        if !chunk_model.is_empty()
            && chunk_model != model.id
            && state.output.response_model.is_none()
        {
            state.output.response_model = Some(chunk_model.to_string());
        }
    }

    // Upstream lines 562-573: chunk-level usage, with the Moonshot-style
    // choice-level fallback.
    let mut had_chunk_usage = false;
    if let Some(usage) = chunk.get("usage").filter(|value| !value.is_null()) {
        state.output.usage = parse_chunk_usage(usage, model);
        had_chunk_usage = true;
    }
    let choice = chunk
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .filter(|choice| choice.is_object());
    let Some(choice) = choice else {
        return Ok(());
    };
    if !had_chunk_usage {
        if let Some(usage) = choice.get("usage").filter(|value| value.is_object()) {
            state.output.usage = parse_chunk_usage(usage, model);
        }
    }

    // Upstream lines 575-583: finish reason mapping + raw preservation. The
    // truthiness guard skips null and empty finish reasons.
    if let Some(finish_reason) = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    {
        state.output.raw_stop_reason = Some(finish_reason.to_string());
        let (stop_reason, error_message) = map_stop_reason(finish_reason);
        state.output.stop_reason = stop_reason;
        if let Some(message) = error_message {
            state.output.error_message = Some(message);
        }
        state.has_finish_reason = true;
    }

    let Some(delta) = choice.get("delta").filter(|delta| delta.is_object()) else {
        return Ok(());
    };

    // Upstream lines 586-599: content deltas.
    if let Some(content) = delta
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
    {
        let content_index = ensure_text_block(state, tx).await;
        if let AssistantBlock::Text(text) = &mut state.output.content[content_index] {
            text.text.push_str(content);
        }
        let _ = tx
            .send(AssistantMessageEvent::TextDelta {
                content_index,
                delta: content.to_string(),
            })
            .await;
    }

    // Upstream lines 601-632: first non-empty reasoning field wins; the block
    // signature is the field name (with the opencode-go special case).
    const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];
    let mut found_reasoning: Option<(&str, &str)> = None;
    for field in REASONING_FIELDS {
        if let Some(value) = delta
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            found_reasoning = Some((field, value));
            break;
        }
    }
    if let Some((field, value)) = found_reasoning {
        let signature = match (model.provider.as_str(), field) {
            ("opencode-go", "reasoning") => "reasoning_content",
            _ => field,
        };
        let content_index = ensure_thinking_block(state, signature, tx).await;
        if let AssistantBlock::Thinking(thinking) = &mut state.output.content[content_index] {
            thinking.thinking.push_str(value);
        }
        let _ = tx
            .send(AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta: value.to_string(),
            })
            .await;
    }

    // Upstream lines 634-662: tool call deltas.
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let content_index = ensure_tool_call_block(state, tool_call, tx).await?;

            // Caller backfills (lines 637-644): id and name on later chunks.
            if let Some(id) = tool_call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                let block_id_empty = matches!(
                    state.output.content.get(content_index),
                    Some(AssistantBlock::ToolCall(call)) if call.id.is_empty()
                );
                if block_id_empty {
                    if let AssistantBlock::ToolCall(call) = &mut state.output.content[content_index]
                    {
                        call.id = id.to_string();
                    }
                    state.tool_by_id.insert(id.to_string(), content_index);
                }
            }
            let name = tool_call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .or_else(|| tool_call.pointer("/custom/name").and_then(Value::as_str))
                .unwrap_or("");
            if !name.is_empty() {
                let block_name_empty = matches!(
                    state.output.content.get(content_index),
                    Some(AssistantBlock::ToolCall(call)) if call.name.is_empty()
                );
                if block_name_empty {
                    if let AssistantBlock::ToolCall(call) = &mut state.output.content[content_index]
                    {
                        call.name = name.to_string();
                    }
                }
            }

            let mut delta_text = String::new();
            if let Some(arguments) = tool_call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .filter(|arguments| !arguments.is_empty())
            {
                delta_text = arguments.to_string();
                let acc = state
                    .tool_accs
                    .get_mut(&content_index)
                    .ok_or_else(|| "Missing tool call accumulator".to_string())?;
                acc.partial_args.push_str(arguments);
                let parsed = parse_streaming_json(&acc.partial_args.clone());
                if let AssistantBlock::ToolCall(call) = &mut state.output.content[content_index] {
                    call.arguments = parsed;
                }
            } else if let Some(input) = tool_call
                .pointer("/custom/input")
                .and_then(Value::as_str)
                .filter(|input| !input.is_empty())
            {
                let next_input = format!(
                    "{}{}",
                    get_custom_tool_call_input(state, content_index),
                    input
                );
                if let Some(delta) =
                    append_custom_tool_call_input(state, content_index, &next_input, false)?
                {
                    delta_text = delta;
                }
            }
            let _ = tx
                .send(AssistantMessageEvent::ToolcallDelta {
                    content_index,
                    delta: delta_text,
                })
                .await;
        }
    }

    // Upstream lines 664-675: reasoning_details replay metadata.
    if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
        for detail in details {
            if !is_openai_reasoning_detail(detail) {
                continue;
            }
            let _ = ensure_thinking_block(state, "", tx).await;
            let streamed = state
                .streamed_reasoning_details
                .get_or_insert_with(Vec::new);
            append_openai_reasoning_detail(streamed, detail);
        }
    }
    Ok(())
}

/// Upstream `mapStopReason` (lines 1552-1576). `null` never reaches it
/// upstream (the truthiness guard skips it), so only strings map here.
fn map_stop_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
        "function_call" | "tool_calls" => (StopReason::ToolUse, None),
        "content_filter" => (
            StopReason::Error,
            Some("Provider finish_reason: content_filter".to_string()),
        ),
        "network_error" => (
            StopReason::Error,
            Some("Provider finish_reason: network_error".to_string()),
        ),
        other => (
            StopReason::Error,
            Some(format!("Provider finish_reason: {other}")),
        ),
    }
}

/// First non-nullish value wins, mirroring the upstream `??` chain (a JSON
/// `0` stays; `null` and absent fall through).
fn first_present<'a>(values: [Option<&'a Value>; 3]) -> Option<&'a Value> {
    let mut found: Option<&'a Value> = None;
    for value in values.into_iter().flatten() {
        if !value.is_null() {
            found = Some(value);
            break;
        }
    }
    found
}

/// Upstream `parseChunkUsage` (lines 1509-1550): documented OpenAI /
/// OpenRouter / DeepSeek / Kimi cache-read placements plus the OpenRouter
/// separate cache-write count; `calculateCost` applied in place.
fn parse_chunk_usage(raw: &Value, model: &Model) -> Usage {
    let token_count = |value: Option<&Value>| -> u64 {
        match value {
            Some(Value::Number(number)) => number
                .as_u64()
                .or_else(|| number.as_f64().map(|value| value.max(0.0) as u64))
                .unwrap_or(0),
            _ => 0,
        }
    };
    // First non-nullish wins, mirroring the `??` chain (0 stays 0).
    let details = raw
        .get("prompt_tokens_details")
        .filter(|value| value.is_object());
    let cache_read = first_present([
        details.and_then(|details| details.get("cached_tokens")),
        raw.get("prompt_cache_hit_tokens"),
        raw.get("cached_tokens"),
    ])
    .map_or(0, |value| token_count(Some(value)));
    let cache_write = details
        .and_then(|details| details.get("cache_write_tokens"))
        .map_or(0, |value| token_count(Some(value)));
    let prompt_tokens = token_count(raw.get("prompt_tokens"));
    let output = token_count(raw.get("completion_tokens"));
    let reasoning = raw
        .get("completion_tokens_details")
        .filter(|value| value.is_object())
        .and_then(|details| details.get("reasoning_tokens"))
        .map_or(0, |value| token_count(Some(value)));

    let input = prompt_tokens
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
    let mut usage = Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: Some(reasoning),
        total_tokens: input
            .saturating_add(output)
            .saturating_add(cache_read)
            .saturating_add(cache_write),
        cost: UsageCost::default(),
    };
    calculate_cost(model, &mut usage);
    usage
}

async fn ensure_text_block(
    state: &mut StreamState,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> usize {
    if let Some(index) = state.text_index {
        return index;
    }
    let index = state.output.content.len();
    state.output.content.push(AssistantBlock::Text(
        crate::ai::types::content::TextContent {
            text: String::new(),
            text_signature: None,
        },
    ));
    state.text_index = Some(index);
    let _ = tx
        .send(AssistantMessageEvent::TextStart {
            content_index: index,
        })
        .await;
    index
}

async fn ensure_thinking_block(
    state: &mut StreamState,
    signature: &str,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> usize {
    if let Some(index) = state.thinking_index {
        return index;
    }
    let index = state.output.content.len();
    state
        .output
        .content
        .push(AssistantBlock::Thinking(ThinkingContent {
            thinking: String::new(),
            thinking_signature: Some(signature.to_string()),
            redacted: None,
        }));
    state.thinking_index = Some(index);
    let _ = tx
        .send(AssistantMessageEvent::ThinkingStart {
            content_index: index,
        })
        .await;
    index
}

/// Upstream `ensureToolCallBlock` (lines 494-551): index/id lookup, creation
/// with grammar/custom input setup, and the in-place backfills.
/// Malformed-stream guard on `tool_calls[].index`. Upstream keeps
/// `toolCallBlocksByIndex` as a JS `Map`, so a hostile index (e.g. 1e15) is
/// only a map key there. This port keys a `HashMap` too, so no large
/// allocation is possible either, but an index far beyond any real parallel
/// tool-call count signals a corrupt or hostile stream: reject it with an
/// error event (the `Err` propagates to `run_stream_task`, which emits
/// `AssistantMessageEvent::Error`) instead of accumulating blocks keyed by
/// garbage.
const MAX_TOOL_CALL_WIRE_INDEX: u64 = 1024;

async fn ensure_tool_call_block(
    state: &mut StreamState,
    tool_call: &Value,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<usize, String> {
    let raw_index = tool_call.get("index").and_then(Value::as_u64);
    if let Some(index) = raw_index {
        if index > MAX_TOOL_CALL_WIRE_INDEX {
            return Err(format!(
                "Malformed tool_calls delta: index {index} exceeds {MAX_TOOL_CALL_WIRE_INDEX}"
            ));
        }
    }
    let stream_index = raw_index;
    let name = tool_call
        .pointer("/function/name")
        .and_then(Value::as_str)
        .or_else(|| tool_call.pointer("/custom/name").and_then(Value::as_str))
        .unwrap_or("");
    let mut found: Option<usize> =
        stream_index.and_then(|index| state.tool_by_wire.get(&index).copied());
    if found.is_none() {
        if let Some(id) = tool_call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            found = state.tool_by_id.get(id).copied();
        }
    }
    let content_index = match found {
        Some(content_index) => content_index,
        None => {
            // Creation path (lines 501-530).
            let custom_present = tool_call
                .get("custom")
                .is_some_and(|value| !value.is_null());
            let function_absent = tool_call
                .get("function")
                .is_none_or(|value| value.is_null());
            let custom_input_property = if custom_present && function_absent {
                Some(
                    state
                        .grammar_props
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| "input".to_string()),
                )
            } else {
                None
            };
            let id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let mut arguments = Map::new();
            if let Some(property) = &custom_input_property {
                arguments.insert(property.clone(), Value::String(String::new()));
            }
            let index = state.output.content.len();
            state
                .output
                .content
                .push(AssistantBlock::ToolCall(ToolCall {
                    id,
                    name: name.to_string(),
                    arguments: Value::Object(arguments),
                    thought_signature: None,
                    namespace: None,
                }));
            let custom_input = custom_input_property.map(|property| CustomInput {
                property,
                buf: GrammarBuf::default(),
            });
            state.tool_accs.insert(
                index,
                ToolAcc {
                    stream_index,
                    partial_args: String::new(),
                    custom_input,
                },
            );
            if let Some(stream_index) = stream_index {
                state.tool_by_wire.insert(stream_index, index);
            }
            if let Some(id) = tool_call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                state.tool_by_id.insert(id.to_string(), index);
            }
            let _ = tx
                .send(AssistantMessageEvent::ToolcallStart {
                    content_index: index,
                })
                .await;
            return Ok(index);
        }
    };

    // Backfills for an existing block (lines 531-550).
    if let Some(stream_index) = stream_index {
        let unregistered = state
            .tool_accs
            .get(&content_index)
            .and_then(|acc| acc.stream_index)
            .is_none();
        if unregistered {
            if let Some(acc) = state.tool_accs.get_mut(&content_index) {
                acc.stream_index = Some(stream_index);
            }
            state.tool_by_wire.insert(stream_index, content_index);
        }
    }
    if let Some(id) = tool_call
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        state.tool_by_id.insert(id.to_string(), content_index);
    }
    if !name.is_empty() {
        if let Some(AssistantBlock::ToolCall(call)) = state.output.content.get_mut(content_index) {
            if call.name.is_empty() {
                call.name = name.to_string();
            }
        }
    }
    let custom_present = tool_call
        .get("custom")
        .is_some_and(|value| !value.is_null());
    let function_absent = tool_call
        .get("function")
        .is_none_or(|value| value.is_null());
    if custom_present && function_absent {
        let has_custom_input = state
            .tool_accs
            .get(&content_index)
            .and_then(|acc| acc.custom_input.as_ref())
            .is_some();
        if !has_custom_input {
            let block_name = match state.output.content.get(content_index) {
                Some(AssistantBlock::ToolCall(call)) => call.name.clone(),
                _ => String::new(),
            };
            let property = state
                .grammar_props
                .get(&block_name)
                .cloned()
                .unwrap_or_else(|| "input".to_string());
            let mut arguments = Map::new();
            arguments.insert(property.clone(), Value::String(String::new()));
            if let Some(AssistantBlock::ToolCall(call)) =
                state.output.content.get_mut(content_index)
            {
                call.arguments = Value::Object(arguments);
            }
            if let Some(acc) = state.tool_accs.get_mut(&content_index) {
                acc.partial_args = String::new();
                acc.custom_input = Some(CustomInput {
                    property,
                    buf: GrammarBuf::default(),
                });
            }
        }
    }
    Ok(content_index)
}

/// Upstream `getCustomToolCallInput` (lines 405-410).
fn get_custom_tool_call_input(state: &StreamState, content_index: usize) -> String {
    let Some(property) = state
        .tool_accs
        .get(&content_index)
        .and_then(|acc| acc.custom_input.as_ref())
        .map(|custom| custom.property.clone())
    else {
        return String::new();
    };
    match state.output.content.get(content_index) {
        Some(AssistantBlock::ToolCall(call)) => call
            .arguments
            .get(&property)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// Upstream `appendGrammarToolInputJsonDelta`
/// (`api/constrained-sampling.ts:157-189`): wraps the raw custom input in a
/// streamed JSON object `{"<property>":"..."}`. Shared with the
/// openai-responses port.
pub(crate) fn append_grammar_tool_input_json_delta(
    buffer: &mut GrammarBuf,
    input_property: &str,
    next_input: &str,
    close: bool,
) -> Result<Option<String>, String> {
    if buffer.closed {
        if close && next_input == buffer.input {
            return Ok(None);
        }
        return Err(format!(
            "grammar tool input for property \"{input_property}\" changed after it was closed"
        ));
    }
    if !next_input.starts_with(&buffer.input) {
        return Err(format!(
            "grammar tool input for property \"{input_property}\" changed non-monotonically"
        ));
    }
    let input_delta = &next_input[buffer.input.len()..];
    if !close && input_delta.is_empty() {
        return Ok(None);
    }
    let mut delta = String::new();
    if !buffer.started {
        delta.push('{');
        delta.push_str(&serde_json::to_string(input_property).expect("property name serializes"));
        delta.push_str(":\"");
        buffer.started = true;
    }
    let escaped = serde_json::to_string(input_delta).expect("delta string serializes");
    delta.push_str(&escaped[1..escaped.len() - 1]);
    buffer.input = next_input.to_string();
    if close {
        delta.push_str("\"}");
        buffer.closed = true;
    }
    Ok(Some(delta))
}

/// Upstream `appendCustomToolCallInput` (lines 411-426): appends through the
/// grammar buffer and mirrors the raw input into `arguments`.
fn append_custom_tool_call_input(
    state: &mut StreamState,
    content_index: usize,
    next_input: &str,
    close: bool,
) -> Result<Option<String>, String> {
    let Some(property) = state
        .tool_accs
        .get_mut(&content_index)
        .and_then(|acc| acc.custom_input.as_mut())
        .map(|custom| custom.property.clone())
    else {
        return Ok(None);
    };
    let delta = {
        let acc = state
            .tool_accs
            .get_mut(&content_index)
            .ok_or_else(|| "Missing tool call accumulator".to_string())?;
        let custom = acc
            .custom_input
            .as_mut()
            .ok_or_else(|| "Missing custom tool input".to_string())?;
        append_grammar_tool_input_json_delta(&mut custom.buf, &property, next_input, close)?
    };
    let mut arguments = Map::new();
    arguments.insert(property, Value::String(next_input.to_string()));
    if let Some(AssistantBlock::ToolCall(call)) = state.output.content.get_mut(content_index) {
        call.arguments = Value::Object(arguments);
    }
    Ok(delta)
}

/// Upstream `appendOpenAIReasoningDetail` (lines 252-266): consecutive
/// text/summary deltas merge; encrypted entries stay discrete.
fn append_openai_reasoning_detail(details: &mut Vec<Value>, detail: &Value) {
    let detail_type = detail.get("type").and_then(Value::as_str);
    if let Some(last) = details.last_mut() {
        let last_type = last.get("type").and_then(Value::as_str);
        let mergeable = (detail_type == Some("reasoning.text")
            && last_type == Some("reasoning.text"))
            || (detail_type == Some("reasoning.summary") && last_type == Some("reasoning.summary"));
        if mergeable {
            let text_field = if detail_type == Some("reasoning.text") {
                "text"
            } else {
                "summary"
            };
            if let Some(object) = last.as_object_mut() {
                let combined = match (
                    object.get(text_field).and_then(Value::as_str),
                    detail.get(text_field).and_then(Value::as_str),
                ) {
                    (Some(existing), Some(incoming)) => format!("{existing}{incoming}"),
                    (Some(existing), None) => existing.to_string(),
                    (None, Some(incoming)) => incoming.to_string(),
                    (None, None) => String::new(),
                };
                object.insert(text_field.to_string(), Value::String(combined));
                // `signature ||=`: only a non-empty string replaces.
                if object
                    .get("signature")
                    .and_then(Value::as_str)
                    .is_none_or(|signature| signature.is_empty())
                {
                    if let Some(signature) = detail
                        .get("signature")
                        .and_then(Value::as_str)
                        .filter(|signature| !signature.is_empty())
                    {
                        object.insert("signature".to_string(), json!(signature));
                    }
                }
                fill_missing_common_reasoning_detail_fields(object, detail);
            }
            return;
        }
    }
    details.push(detail.clone());
}

/// Upstream `fillMissingCommonReasoningDetailFields` (lines 243-250):
/// `id ??=`, `format ||=`, `index ??=` on the merged entry.
fn fill_missing_common_reasoning_detail_fields(target: &mut Map<String, Value>, source: &Value) {
    if target.get("id").is_none_or(|value| value.is_null()) {
        if let Some(id) = source.get("id").filter(|value| !value.is_null()) {
            target.insert("id".to_string(), id.clone());
        }
    }
    let format_falsy = target
        .get("format")
        .and_then(Value::as_str)
        .is_none_or(|format| format.is_empty());
    if format_falsy {
        if let Some(format) = source
            .get("format")
            .and_then(Value::as_str)
            .filter(|format| !format.is_empty())
        {
            target.insert("format".to_string(), json!(format));
        }
    }
    if target.get("index").is_none_or(|value| value.is_null()) {
        if let Some(index) = source.get("index").filter(|value| !value.is_null()) {
            target.insert("index".to_string(), index.clone());
        }
    }
}

/// Upstream `finishBlock` (lines 427-473): close one block with its
/// authoritative end event.
async fn finish_block(
    state: &mut StreamState,
    content_index: usize,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
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
            // Upstream `applyStreamedReasoningDetails` (lines 329-333).
            if let Some(details) = &state.streamed_reasoning_details {
                if let Ok(signature) = serde_json::to_string(details) {
                    if let Some(AssistantBlock::Thinking(thinking)) =
                        state.output.content.get_mut(content_index)
                    {
                        thinking.thinking_signature = Some(signature);
                    }
                }
            }
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
            let has_custom_input = state
                .tool_accs
                .get(&content_index)
                .and_then(|acc| acc.custom_input.as_ref())
                .is_some();
            if has_custom_input {
                // Upstream lines 448-457: flush and close the grammar buffer.
                let input = get_custom_tool_call_input(state, content_index);
                if let Some(delta) =
                    append_custom_tool_call_input(state, content_index, &input, true)?
                {
                    let _ = tx
                        .send(AssistantMessageEvent::ToolcallDelta {
                            content_index,
                            delta,
                        })
                        .await;
                }
            } else {
                // Upstream line 459: parse the accumulated fragments.
                let partial_args = state
                    .tool_accs
                    .get(&content_index)
                    .map(|acc| acc.partial_args.clone())
                    .unwrap_or_default();
                let parsed = parse_streaming_json(&partial_args);
                if let Some(AssistantBlock::ToolCall(call)) =
                    state.output.content.get_mut(content_index)
                {
                    call.arguments = parsed;
                }
            }
            let tool_call = match &state.output.content[content_index] {
                AssistantBlock::ToolCall(call) => call.clone(),
                _ => return Ok(()),
            };
            let _ = tx
                .send(AssistantMessageEvent::ToolcallEnd {
                    content_index,
                    tool_call,
                })
                .await;
        }
        None => {}
    }
    Ok(())
}

// ---- parseStreamingJson port (utils/json-parse.ts) ----

/// Approximation of the `partial-json` fallback: close an open string and any
/// open containers, complete a truncated `true`/`false`/`null` literal, turn a
/// dangling `:` into a `null` value, and drop a trailing comma. Returns `None`
/// when no completion is plausible; callers fall back to `{}`.
fn complete_partial_json(input: &str) -> Option<String> {
    let mut in_string = false;
    let mut escaped = false;
    let mut stack: Vec<char> = Vec::new();
    let mut last_significant = '\0';
    // Byte offset after the last structural character / completed token, so
    // the trailing text outside strings can be checked for a truncated
    // `true`/`false`/`null` literal.
    let mut token_start = 0usize;
    for (index, character) in input.char_indices() {
        if in_string {
            match character {
                '\\' => escaped = !escaped,
                '"' if !escaped => {
                    in_string = false;
                    last_significant = '"';
                    token_start = index + 1;
                }
                _ => escaped = false,
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                escaped = false;
                last_significant = '"';
            }
            '{' => {
                stack.push('}');
                last_significant = '{';
                token_start = index + 1;
            }
            '[' => {
                stack.push(']');
                last_significant = '[';
                token_start = index + 1;
            }
            '}' | ']' => {
                stack.pop();
                last_significant = character;
                token_start = index + 1;
            }
            ':' | ',' => {
                last_significant = character;
                token_start = index + 1;
            }
            // `index` is a byte offset: advance by the full UTF-8 length so
            // multi-byte whitespace (U+00A0, U+3000, ...) cannot leave
            // token_start inside a character and panic the later slice.
            character if character.is_whitespace() => token_start = index + character.len_utf8(),
            character => last_significant = character,
        }
    }
    let mut completed = input.to_string();
    if in_string {
        if escaped {
            // A dangling escape cannot be closed safely.
            return None;
        }
        completed.push('"');
        return Some(close_stack(completed, stack));
    }
    // Truncated literal completion ("tru", "fals", "nul", ...).
    const LITERALS: [&str; 3] = ["true", "false", "null"];
    let trailing = &completed[token_start.min(completed.len())..];
    if !trailing.is_empty() {
        if let Some(literal) = LITERALS
            .into_iter()
            .find(|literal| literal.starts_with(trailing) && trailing.len() < literal.len())
        {
            completed.push_str(&literal[trailing.len()..]);
            return Some(close_stack(completed, stack));
        }
    }
    match last_significant {
        ':' => completed.push_str("null"),
        ',' => {
            // Drop the trailing comma (partial-json yields the complete
            // elements so far).
            while completed.ends_with(',') {
                completed.pop();
            }
        }
        _ => {}
    }
    Some(close_stack(completed, stack))
}

fn close_stack(mut completed: String, stack: Vec<char>) -> String {
    for closer in stack.into_iter().rev() {
        completed.push(closer);
    }
    completed
}

/// Upstream `parseStreamingJson` (`utils/json-parse.ts:103-125`): never
/// throws; falls back to an empty object.
pub(crate) fn parse_streaming_json(partial: &str) -> Value {
    if partial.trim().is_empty() {
        return json!({});
    }
    if let Ok(value) = serde_json::from_str(partial) {
        return value;
    }
    let repaired = repair_json(partial);
    if let Ok(value) = serde_json::from_str(&repaired) {
        return value;
    }
    if let Some(completed) = complete_partial_json(partial) {
        if let Ok(value) = serde_json::from_str(&completed) {
            return value;
        }
        if let Ok(value) = serde_json::from_str(&repair_json(&completed)) {
            return value;
        }
    }
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::ai::api::openai_completions::request::RequestAssembly;
    use crate::ai::api::{abort_test_support::stalled_sse_server, REQUEST_ABORTED};
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::content::TextContent;
    use crate::ai::types::events::{ErrorReason, PartialAssistant, SuccessReason};
    use crate::ai::types::message::{
        AssistantBlock, Message, StringOrBlocks, ToolResultMessage, UserMessage,
    };
    use crate::ai::types::primitives::{ModelCost, StopReason, ToolChoice, Usage, UsageCost};
    use crate::ai::types::tool::{ConstrainedSampling, GrammarFormat, GrammarSampling, Tool};

    const TS: i64 = 1758240000000;

    fn base_model() -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "gpt-test".to_string(),
            name: "Test Model".to_string(),
            api: "openai-completions".to_string(),
            provider: "openai".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![crate::ai::types::ModelInput::Text],
            cost: ModelCost::default(),
            context_window: 100_000,
            max_tokens: 4096,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    fn priced_model() -> Model {
        let mut model = base_model();
        model.cost = ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.15,
            cache_write: 7.5,
            tiers: None,
        };
        model
    }

    fn cfg(server: &wiremock::MockServer) -> ProviderConfig {
        ProviderConfig {
            base_url: format!("{}/v1", server.uri()),
            api_key: "k".to_string(),
            max_tokens: 8192,
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
            content: StringOrBlocks::Text(text.into()),
            timestamp: TS,
        })
    }

    fn tool(name: &str) -> Tool {
        Tool {
            name: name.into(),
            description: "A tool".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            constrained_sampling: None,
        }
    }

    fn grammar_tool(name: &str, property: &str) -> Tool {
        Tool {
            name: name.into(),
            description: "A grammar tool".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { property: {"type": "string"} },
                "required": [property]
            }),
            constrained_sampling: Some(ConstrainedSampling::Grammar(GrammarSampling {
                variants: [(GrammarFormat::Lark, "grammar".to_string())]
                    .into_iter()
                    .collect(),
            })),
        }
    }

    // ---- SSE helpers ----

    fn sse(body: &str) -> wiremock::ResponseTemplate {
        wiremock::ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body.to_string())
    }

    fn data_line(json: serde_json::Value) -> String {
        format!("data: {json}\n\n")
    }

    fn done_line() -> String {
        "data: [DONE]\n\n".to_string()
    }

    fn content_chunk(text: &str) -> serde_json::Value {
        serde_json::json!({"id": "chatcmpl-1", "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]})
    }

    fn delta_chunk(delta: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"id": "chatcmpl-1", "choices": [{"index": 0, "delta": delta, "finish_reason": null}]})
    }

    fn finish_chunk(finish_reason: &str) -> serde_json::Value {
        serde_json::json!({"id": "chatcmpl-1", "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}]})
    }

    async fn collect_simple(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = OpenAiCompletions;
        let mut rx = api.stream_simple(&cfg(server), model, ctx, options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    async fn collect_stream(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = OpenAiCompletions;
        let mut rx = api.stream(&cfg(server), model, ctx, options);
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

    // ---- 1. text deltas with canonical companions ----

    #[tokio::test]
    async fn text_deltas_with_start_end_companions_and_done() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}{}",
            data_line(content_chunk("Hello,")),
            data_line(content_chunk(" world")),
            data_line(finish_chunk("stop")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        assert_eq!(
            event_types(&events),
            [
                "start",
                "text_start",
                "text_delta",
                "text_delta",
                "text_end",
                "done"
            ]
        );
        match &events[0] {
            AssistantMessageEvent::Start { message } => {
                assert_eq!(message.api, "openai-completions");
                assert_eq!(message.provider, "openai");
                assert_eq!(message.model, "gpt-test");
                assert!(message.content.is_empty());
                assert_eq!(message.stop_reason, StopReason::Pending);
                assert!(message.timestamp > 0);
                assert_eq!(message.usage, Usage::default());
            }
            other => panic!("expected Start, got {other:?}"),
        }
        assert_eq!(
            events[1],
            AssistantMessageEvent::TextStart { content_index: 0 }
        );
        assert_eq!(
            events[2],
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Hello,".into()
            }
        );
        assert_eq!(
            events[3],
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: " world".into()
            }
        );
        assert_eq!(
            events[4],
            AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: "Hello, world".into()
            }
        );
        match &events[5] {
            AssistantMessageEvent::Done { reason, message } => {
                assert_eq!(*reason, SuccessReason::Stop);
                assert_eq!(
                    message.content,
                    vec![AssistantBlock::Text(TextContent {
                        text: "Hello, world".into(),
                        text_signature: None
                    })]
                );
                assert_eq!(message.stop_reason, StopReason::Stop);
                assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));
                assert_eq!(message.error_message, None);
                assert_eq!(message.usage, Usage::default());
            }
            other => panic!("expected Done, got {other:?}"),
        }

        // The event stream reconstructs through the M2a reducer to exactly the
        // Done message.
        let partial = apply_all(&events);
        match &events[5] {
            AssistantMessageEvent::Done { message, .. } => {
                assert_eq!(partial.message(), Some(message));
            }
            _ => unreachable!(),
        }
        assert!(partial.is_terminal());
    }

    // ---- 2. reasoning_content deltas ----

    #[tokio::test]
    async fn reasoning_content_deltas_create_thinking_block_with_companions() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}{}{}",
            data_line(delta_chunk(json!({"reasoning_content": "why"}))),
            data_line(delta_chunk(json!({"reasoning_content": " though"}))),
            data_line(delta_chunk(json!({"content": "answer"}))),
            data_line(finish_chunk("stop")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        assert_eq!(
            event_types(&events),
            [
                "start",
                "thinking_start",
                "thinking_delta",
                "thinking_delta",
                "text_start",
                "text_delta",
                "thinking_end",
                "text_end",
                "done"
            ]
        );
        // Block 0: thinking; block 1: text. The finish pass ends blocks in
        // content order.
        assert_eq!(
            events[1],
            AssistantMessageEvent::ThinkingStart { content_index: 0 }
        );
        assert_eq!(
            events[2],
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "why".into()
            }
        );
        assert_eq!(
            events[4],
            AssistantMessageEvent::TextStart { content_index: 1 }
        );
        assert_eq!(
            events[5],
            AssistantMessageEvent::TextDelta {
                content_index: 1,
                delta: "answer".into()
            }
        );
        assert_eq!(
            events[6],
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                content: "why though".into()
            }
        );
        assert_eq!(
            events[7],
            AssistantMessageEvent::TextEnd {
                content_index: 1,
                content: "answer".into()
            }
        );
        match &events[8] {
            AssistantMessageEvent::Done { reason, message } => {
                assert_eq!(*reason, SuccessReason::Stop);
                assert_eq!(
                    message.content,
                    vec![
                        AssistantBlock::Thinking(crate::ai::types::ThinkingContent {
                            thinking: "why though".into(),
                            thinking_signature: Some("reasoning_content".into()),
                            redacted: None,
                        }),
                        AssistantBlock::Text(TextContent {
                            text: "answer".into(),
                            text_signature: None
                        }),
                    ]
                );
            }
            other => panic!("expected Done, got {other:?}"),
        }
        apply_all(&events);
    }

    #[tokio::test]
    async fn reasoning_field_preference_first_non_empty_wins() {
        let server = wiremock::MockServer::start().await;
        // First non-empty field per chunk: reasoning_content beats reasoning;
        // later chunks append to the same block regardless of the field used.
        let body = format!(
            "{}{}{}{}",
            data_line(delta_chunk(
                json!({"reasoning_content": "a", "reasoning": "b"})
            )),
            data_line(delta_chunk(json!({"reasoning": "c"}))),
            data_line(finish_chunk("stop")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        match events.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                assert_eq!(
                    message.content,
                    vec![AssistantBlock::Thinking(
                        crate::ai::types::ThinkingContent {
                            thinking: "ac".into(),
                            thinking_signature: Some("reasoning_content".into()),
                            redacted: None,
                        }
                    )]
                );
            }
            other => panic!("expected Done, got {other:?}"),
        }
        apply_all(&events);
    }

    // ---- 3. tool calls: multi-index accumulation with fragments ----

    #[tokio::test]
    async fn tool_calls_multi_index_accumulation_with_fragments() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}{}{}",
            data_line(delta_chunk(json!({"tool_calls": [
                {"index": 0, "id": "call_a", "type": "function", "function": {"name": "read", "arguments": "{\"pa"}}
            ]}))),
            data_line(delta_chunk(json!({"tool_calls": [
                {"index": 0, "function": {"arguments": "th\":\"a.txt\"}"}},
                {"index": 1, "id": "call_b", "type": "function", "function": {"name": "write"}}
            ]}))),
            data_line(delta_chunk(json!({"tool_calls": [
                {"index": 1, "function": {"arguments": "{\"x\":1}"}}
            ]}))),
            data_line(finish_chunk("tool_calls")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        assert_eq!(
            event_types(&events),
            [
                "start",
                "toolcall_start",
                "toolcall_delta",
                "toolcall_delta",
                "toolcall_start",
                "toolcall_delta",
                "toolcall_delta",
                "toolcall_end",
                "toolcall_end",
                "done"
            ]
        );
        assert_eq!(
            events[1],
            AssistantMessageEvent::ToolcallStart { content_index: 0 }
        );
        assert_eq!(
            events[2],
            AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: "{\"pa".into()
            }
        );
        assert_eq!(
            events[3],
            AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: "th\":\"a.txt\"}".into()
            }
        );
        assert_eq!(
            events[4],
            AssistantMessageEvent::ToolcallStart { content_index: 1 }
        );
        // An entry without arguments still pushes a (empty) delta event, like
        // upstream.
        assert_eq!(
            events[5],
            AssistantMessageEvent::ToolcallDelta {
                content_index: 1,
                delta: String::new()
            }
        );
        assert_eq!(
            events[6],
            AssistantMessageEvent::ToolcallDelta {
                content_index: 1,
                delta: "{\"x\":1}".into()
            }
        );
        match &events[7] {
            AssistantMessageEvent::ToolcallEnd {
                content_index,
                tool_call,
            } => {
                assert_eq!(*content_index, 0);
                assert_eq!(tool_call.id, "call_a");
                assert_eq!(tool_call.name, "read");
                assert_eq!(tool_call.arguments, serde_json::json!({"path": "a.txt"}));
            }
            other => panic!("expected ToolcallEnd, got {other:?}"),
        }
        match &events[8] {
            AssistantMessageEvent::ToolcallEnd {
                content_index,
                tool_call,
            } => {
                assert_eq!(*content_index, 1);
                assert_eq!(tool_call.id, "call_b");
                assert_eq!(tool_call.name, "write");
                assert_eq!(tool_call.arguments, serde_json::json!({"x": 1}));
            }
            other => panic!("expected ToolcallEnd, got {other:?}"),
        }
        match events.last().unwrap() {
            AssistantMessageEvent::Done { reason, .. } => {
                assert_eq!(*reason, SuccessReason::ToolUse)
            }
            other => panic!("expected Done, got {other:?}"),
        }
        apply_all(&events);
    }

    // ---- 3b. malformed tool_calls index guard ----

    /// A `tool_calls[].index` far beyond any real parallel tool-call count is
    /// rejected with an error event instead of accumulating a block keyed by
    /// garbage (upstream's JS `Map` tolerates it; this port clamps — see
    /// `MAX_TOOL_CALL_WIRE_INDEX`).
    #[tokio::test]
    async fn huge_tool_call_index_is_rejected_with_error_event() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}",
            data_line(delta_chunk(json!({"tool_calls": [
                {"index": 5_000_000_000u64, "id": "call_x", "type": "function", "function": {"name": "read", "arguments": "{}"}}
            ]}))),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        match events.last().unwrap() {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(*reason, ErrorReason::Error);
                assert_eq!(error.stop_reason, StopReason::Error);
                let message = error.error_message.as_deref().unwrap_or_default();
                assert!(
                    message.contains("Malformed tool_calls delta"),
                    "unexpected error message: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// An index within the guard keeps flowing normally (boundary check).
    #[tokio::test]
    async fn tool_call_index_at_guard_boundary_accumulates() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}",
            data_line(delta_chunk(json!({"tool_calls": [
                {"index": 1024, "id": "call_hi", "type": "function", "function": {"name": "read", "arguments": "{}"}}
            ]}))),
            data_line(finish_chunk("tool_calls")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        match events.last().unwrap() {
            AssistantMessageEvent::Done { reason, .. } => {
                assert_eq!(*reason, SuccessReason::ToolUse)
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // ---- 4. finish reason mapping + raw stop reason ----

    #[tokio::test]
    async fn finish_reason_mapping_and_raw_stop_reason_preserved() {
        for (finish, expected_reason) in [
            ("stop", SuccessReason::Stop),
            ("end", SuccessReason::Stop),
            ("length", SuccessReason::Length),
            ("tool_calls", SuccessReason::ToolUse),
            ("function_call", SuccessReason::ToolUse),
        ] {
            let server = wiremock::MockServer::start().await;
            let body = format!("{}{}", data_line(finish_chunk(finish)), done_line());
            wiremock::Mock::given(wiremock::matchers::method("POST"))
                .respond_with(sse(&body))
                .mount(&server)
                .await;

            let model = base_model();
            let events = collect_simple(
                &server,
                &model,
                &user_ctx(vec![user_msg("hi")]),
                &SimpleStreamOptions::default(),
            )
            .await;
            match events.last().unwrap() {
                AssistantMessageEvent::Done { reason, message } => {
                    assert_eq!(*reason, expected_reason, "finish {finish}");
                    assert_eq!(message.stop_reason, StopReason::from(expected_reason));
                    assert_eq!(message.raw_stop_reason.as_deref(), Some(finish));
                }
                other => panic!("expected Done for finish {finish}, got {other:?}"),
            }
            apply_all(&events);
        }
    }

    #[tokio::test]
    async fn content_filter_finish_reason_becomes_error_event() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}",
            data_line(finish_chunk("content_filter")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        assert_eq!(event_types(&events), ["start", "error"]);
        match events.last().unwrap() {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(*reason, ErrorReason::Error);
                assert_eq!(error.stop_reason, StopReason::Error);
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("Provider finish_reason: content_filter")
                );
                // Raw stop reason preserved through the error path.
                assert_eq!(error.raw_stop_reason.as_deref(), Some("content_filter"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
        apply_all(&events);
    }

    // ---- 5. supportsFinishReason = false inference ----

    #[tokio::test]
    async fn missing_finish_reason_errors_when_supported() {
        let server = wiremock::MockServer::start().await;
        let body = format!("{}{}", data_line(content_chunk("hi")), done_line());
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        // Upstream pushes the end-of-stream end events (finishBlock loop)
        // before the terminal checks, so text_end precedes the error event.
        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "text_end", "error"]
        );
        match events.last().unwrap() {
            AssistantMessageEvent::Error { error, .. } => {
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("Stream ended without finish_reason")
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
        apply_all(&events);
    }

    #[tokio::test]
    async fn supports_finish_reason_false_infers_stop_and_tool_use() {
        for (with_tool_call, expected_reason) in
            [(false, SuccessReason::Stop), (true, SuccessReason::ToolUse)]
        {
            let server = wiremock::MockServer::start().await;
            let payload = if with_tool_call {
                delta_chunk(json!({"tool_calls": [
                    {"index": 0, "id": "call_a", "type": "function", "function": {"name": "read", "arguments": "{}"}}
                ]}))
            } else {
                content_chunk("hi")
            };
            let body = format!("{}{}", data_line(payload), done_line());
            wiremock::Mock::given(wiremock::matchers::method("POST"))
                .respond_with(sse(&body))
                .mount(&server)
                .await;

            let mut model = base_model();
            model.compat = Some(serde_json::json!({"supportsFinishReason": false}));
            let events = collect_simple(
                &server,
                &model,
                &user_ctx(vec![user_msg("hi")]),
                &SimpleStreamOptions::default(),
            )
            .await;
            match events.last().unwrap() {
                AssistantMessageEvent::Done { reason, message } => {
                    assert_eq!(*reason, expected_reason);
                    assert_eq!(message.stop_reason, StopReason::from(expected_reason));
                    assert_eq!(message.raw_stop_reason, None);
                }
                other => panic!("expected Done, got {other:?}"),
            }
            apply_all(&events);
        }
    }

    // ---- 6. usage mapping ----

    #[tokio::test]
    async fn usage_mapping_with_cache_read_write_reasoning_and_cost() {
        let server = wiremock::MockServer::start().await;
        let usage = serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "prompt_tokens_details": {"cached_tokens": 20, "cache_write_tokens": 10},
            "completion_tokens_details": {"reasoning_tokens": 8}
        });
        let final_chunk = serde_json::json!({"id": "chatcmpl-1", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage});
        let body = format!(
            "{}{}{}",
            data_line(content_chunk("x")),
            data_line(final_chunk),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = priced_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        match events.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                let usage = &message.usage;
                // input = 100 - 20 - 10; reasoning is a subset of output.
                assert_eq!(usage.input, 70);
                assert_eq!(usage.output, 50);
                assert_eq!(usage.cache_read, 20);
                assert_eq!(usage.cache_write, 10);
                assert_eq!(usage.reasoning, Some(8));
                assert_eq!(usage.total_tokens, 150);
                // calculate_cost applied with the model rates.
                let eps = 1e-12;
                assert!(
                    (usage.cost.input - 3.0 / 1e6 * 70.0).abs() < eps,
                    "{:?}",
                    usage.cost
                );
                assert!(
                    (usage.cost.output - 15.0 / 1e6 * 50.0).abs() < eps,
                    "{:?}",
                    usage.cost
                );
                assert!(
                    (usage.cost.cache_read - 0.15 / 1e6 * 20.0).abs() < eps,
                    "{:?}",
                    usage.cost
                );
                assert!(
                    (usage.cost.cache_write - 7.5 / 1e6 * 10.0).abs() < eps,
                    "{:?}",
                    usage.cost
                );
                assert!(
                    (usage.cost.total
                        - usage.cost.input
                        - usage.cost.output
                        - usage.cost.cache_read
                        - usage.cost.cache_write)
                        .abs()
                        < eps,
                    "{:?}",
                    usage.cost
                );
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn usage_fallback_fields_and_choice_level_usage() {
        // DeepSeek prompt_cache_hit_tokens and Kimi top-level cached_tokens.
        for (usage, expected_input, expected_cache_read) in [
            (
                serde_json::json!({"prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 4}),
                6u64,
                4u64,
            ),
            (
                serde_json::json!({"prompt_tokens": 10, "completion_tokens": 5, "cached_tokens": 4}),
                6,
                4,
            ),
            (
                serde_json::json!({"prompt_tokens": 10, "completion_tokens": 5}),
                10,
                0,
            ),
        ] {
            let server = wiremock::MockServer::start().await;
            let final_chunk = serde_json::json!({"id": "chatcmpl-1", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage});
            let body = format!("{}{}", data_line(final_chunk), done_line());
            wiremock::Mock::given(wiremock::matchers::method("POST"))
                .respond_with(sse(&body))
                .mount(&server)
                .await;

            let model = base_model();
            let events = collect_simple(
                &server,
                &model,
                &user_ctx(vec![user_msg("hi")]),
                &SimpleStreamOptions::default(),
            )
            .await;
            match events.last().unwrap() {
                AssistantMessageEvent::Done { message, .. } => {
                    assert_eq!(message.usage.input, expected_input, "{usage}");
                    assert_eq!(message.usage.cache_read, expected_cache_read, "{usage}");
                    assert_eq!(message.usage.output, 5);
                    // totalTokens = input + output + cacheRead + cacheWrite.
                    assert_eq!(
                        message.usage.total_tokens,
                        expected_input + 5 + expected_cache_read
                    );
                }
                other => panic!("expected Done, got {other:?}"),
            }
        }

        // Moonshot-style choice-level usage fallback.
        let server = wiremock::MockServer::start().await;
        let final_chunk = serde_json::json!({"id": "chatcmpl-1", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop", "usage": {"prompt_tokens": 7, "completion_tokens": 3}}]});
        let body = format!("{}{}", data_line(final_chunk), done_line());
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;
        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;
        match events.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                assert_eq!(message.usage.input, 7);
                assert_eq!(message.usage.output, 3);
                assert_eq!(message.usage.total_tokens, 10);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // ---- 7. responseModel / responseId ----

    #[tokio::test]
    async fn response_model_and_response_id_from_chunks() {
        // Routed model id surfaces on responseModel; requested id stays.
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}",
            data_line(
                serde_json::json!({"id": "chatcmpl-1", "model": "anthropic/claude-opus-4.8", "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": null}]})
            ),
            data_line(
                serde_json::json!({"id": "chatcmpl-1", "model": "anthropic/claude-opus-4.8", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 10, "completion_tokens": 5}})
            ),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;
        let mut model = base_model();
        model.id = "openrouter/auto".into();
        model.provider = "openrouter".into();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;
        match events.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                assert_eq!(message.model, "openrouter/auto");
                assert_eq!(
                    message.response_model.as_deref(),
                    Some("anthropic/claude-opus-4.8")
                );
                assert_eq!(message.provider, "openrouter");
                assert_eq!(message.response_id.as_deref(), Some("chatcmpl-1"));
            }
            other => panic!("expected Done, got {other:?}"),
        }

        // Echoing chunks keep responseModel unset.
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}",
            data_line(
                serde_json::json!({"id": "chatcmpl-2", "model": "openrouter/auto", "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": null}]})
            ),
            data_line(
                serde_json::json!({"id": "chatcmpl-2", "model": "openrouter/auto", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
            ),
        );
        let body = format!("{body}{}", done_line());
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;
        match events.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                assert_eq!(message.response_model, None);
            }
            other => panic!("expected Done, got {other:?}"),
        }

        // Empty and missing chunk.model are ignored.
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}{}",
            data_line(
                serde_json::json!({"id": "chatcmpl-3", "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": null}]})
            ),
            data_line(
                serde_json::json!({"id": "chatcmpl-3", "model": "", "choices": [{"index": 0, "delta": {"content": "!"}, "finish_reason": null}]})
            ),
            data_line(
                serde_json::json!({"id": "chatcmpl-3", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
            ),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;
        match events.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                assert_eq!(message.response_model, None);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // ---- 8. errors: pre-start vs mid-stream ----

    #[tokio::test]
    async fn http_error_before_start_carries_status_and_body() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(401)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"error":{"message":"bad key"}}"#),
            )
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        assert_eq!(
            event_types(&events),
            ["error"],
            "pre-start error: no Start event"
        );
        match &events[0] {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(*reason, ErrorReason::Error);
                let message = error.error_message.as_deref().unwrap_or_default();
                assert!(message.contains("401"), "status in message: {message}");
                assert!(message.contains("bad key"), "body in message: {message}");
                assert_eq!(error.stop_reason, StopReason::Error);
                assert_eq!(error.api, "openai-completions");
                assert_eq!(error.provider, "openai");
                assert_eq!(error.model, "gpt-test");
            }
            other => panic!("expected Error, got {other:?}"),
        }
        // Reducer: lone error before start is legal and terminal.
        let partial = apply_all(&events);
        assert!(partial.is_terminal());
        assert_eq!(partial.message().unwrap().stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn missing_api_key_yields_error_before_start() {
        let server = wiremock::MockServer::start().await;
        let api = OpenAiCompletions;
        let request_cfg = ProviderConfig {
            base_url: format!("{}/v1", server.uri()),
            api_key: String::new(),
            max_tokens: 8192,
        };
        let mut rx = api.stream_simple(
            &request_cfg,
            &base_model(),
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        );
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        assert_eq!(event_types(&events), ["error"]);
        match &events[0] {
            AssistantMessageEvent::Error { error, .. } => {
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("No API key for provider: openai")
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mid_stream_error_keeps_partial_content() {
        let server = wiremock::MockServer::start().await;
        let body = format!("{}data: not-json\n\n", data_line(content_chunk("partial")));
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "error"]
        );
        match events.last().unwrap() {
            AssistantMessageEvent::Error { error, .. } => {
                assert_eq!(error.stop_reason, StopReason::Error);
                assert!(error.error_message.is_some());
            }
            other => panic!("expected Error, got {other:?}"),
        }
        // The reducer keeps the partial content across the mid-stream error.
        let partial = apply_all(&events);
        let message = partial.message().unwrap();
        assert_eq!(
            message.content,
            vec![AssistantBlock::Text(TextContent {
                text: "partial".into(),
                text_signature: None
            })]
        );
        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(partial.is_terminal());
    }

    // ---- 9. malformed chunks must not panic ----

    #[tokio::test]
    async fn malformed_chunks_are_skipped_without_panicking() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}{}{}{}{}{}",
            data_line(Value::Null),
            data_line(json!(42)),
            data_line(json!([1, 2])),
            data_line(json!({})),
            data_line(json!({"choices": []})),
            data_line(content_chunk("ok")),
            data_line(finish_chunk("stop")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "text_end", "done"],
            "unexpected events: {events:?}"
        );
        apply_all(&events);
    }

    #[tokio::test]
    async fn malformed_tool_arguments_with_multibyte_whitespace_do_not_panic() {
        // Upstream's parseStreamingJson never throws on malformed accumulated
        // arguments (falls back to {}); the port must do the same. A multi-byte
        // whitespace character (U+3000) outside a JSON string in the trailing
        // fragment must not panic the stream task — a panic would kill the
        // spawned task and the consumer would see channel closure with neither
        // Done nor Error.
        let server = wiremock::MockServer::start().await;
        let fragment = format!("{{\"a\":{}", '\u{3000}');
        let body = format!(
            "{}{}{}",
            data_line(delta_chunk(json!({"tool_calls": [
                {"index": 0, "id": "call_a", "type": "function", "function": {"name": "read", "arguments": fragment}}
            ]}))),
            data_line(finish_chunk("tool_calls")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        // The stream terminates with a terminal event (never channel closure
        // without Done/Error).
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. }) | Some(AssistantMessageEvent::Error { .. })
        ));
        // The parse falls back to {} per the never-throw contract, so the
        // tool call ends with empty arguments and the stream completes.
        match events.last().unwrap() {
            AssistantMessageEvent::Done { reason, message } => {
                assert_eq!(*reason, SuccessReason::ToolUse);
                let calls: Vec<&crate::ai::types::ToolCall> = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::ToolCall(call) => Some(call),
                        _ => None,
                    })
                    .collect();
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "read");
                assert_eq!(calls[0].arguments, serde_json::json!({}));
            }
            other => panic!("expected Done, got {other:?}"),
        }
        apply_all(&events);
    }

    // ---- 10. reasoning_details (upstream oracle) ----

    #[tokio::test]
    async fn reasoning_details_preserved_in_thinking_signature() {
        let server = wiremock::MockServer::start().await;
        let detail = serde_json::json!({"type": "reasoning.encrypted", "id": "call_1", "data": "encrypted-signature"});
        let body = format!(
            "{}{}{}{}",
            data_line(delta_chunk(json!({"reasoning_details": [detail]}))),
            data_line(delta_chunk(json!({"tool_calls": [
                {"index": 0, "id": "call_1", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"README.md\"}"}}
            ]}))),
            data_line(finish_chunk("tool_calls")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let mut model = base_model();
        model.id = "google/gemini-test".into();
        model.provider = "openrouter".into();
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("hi")],
            tools: Some(vec![tool("read")]),
        });
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;

        match events.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                // The thinking block carries the serialized reasoning_details
                // as its signature (structural compare: key order follows
                // serde_json's sorted keys).
                let thinking = message.content.iter().find_map(|block| match block {
                    AssistantBlock::Thinking(thinking) => Some(thinking),
                    _ => None,
                });
                let thinking = thinking.expect("thinking block present");
                assert_eq!(thinking.thinking, "");
                let signature: Value = serde_json::from_str(
                    thinking.thinking_signature.as_deref().unwrap_or_default(),
                )
                .expect("signature is JSON");
                assert_eq!(signature, serde_json::json!([detail]));
                let call = message.content.iter().find_map(|block| match block {
                    AssistantBlock::ToolCall(call) => Some(call),
                    _ => None,
                });
                let call = call.expect("tool call present");
                assert_eq!(call.id, "call_1");
                assert_eq!(call.name, "read");
                assert_eq!(call.arguments, serde_json::json!({"path": "README.md"}));
            }
            other => panic!("expected Done, got {other:?}"),
        }
        apply_all(&events);
    }

    #[tokio::test]
    async fn reasoning_details_consecutive_deltas_merge() {
        let server = wiremock::MockServer::start().await;
        let text_delta = json!({"type": "reasoning.text", "text": "The", "index": 0});
        let text_delta_signed = json!({"type": "reasoning.text", "text": " user wants the time.", "signature": "sha256:text-signature", "format": "openai-responses-v1", "index": 0});
        let summary_delta = json!({"type": "reasoning.summary", "summary": "Looked", "index": 0});
        let summary_delta_formatted = json!({"type": "reasoning.summary", "summary": " up time.", "format": "openai-responses-v1", "index": 0});
        let encrypted =
            json!({"type": "reasoning.encrypted", "id": "call_1", "data": "encrypted-signature"});
        let later_summary = json!({"type": "reasoning.summary", "summary": "After encrypted block.", "format": "openai-responses-v1", "index": 0});
        let body = format!(
            "{}{}{}{}{}{}{}{}",
            data_line(delta_chunk(json!({"reasoning_details": [text_delta]}))),
            data_line(delta_chunk(
                json!({"reasoning_details": [text_delta_signed]})
            )),
            data_line(delta_chunk(json!({"reasoning_details": [summary_delta]}))),
            data_line(delta_chunk(
                json!({"reasoning_details": [summary_delta_formatted]})
            )),
            data_line(delta_chunk(json!({"reasoning_details": [encrypted]}))),
            data_line(delta_chunk(json!({"reasoning_details": [later_summary]}))),
            data_line(finish_chunk("stop")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let events = collect_simple(
            &server,
            &model,
            &user_ctx(vec![user_msg("hi")]),
            &SimpleStreamOptions::default(),
        )
        .await;

        match events.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                let thinking = message.content.iter().find_map(|block| match block {
                    AssistantBlock::Thinking(thinking) => Some(thinking),
                    _ => None,
                });
                let thinking = thinking.expect("thinking block present");
                assert_eq!(thinking.thinking, "");
                let signature: Value = serde_json::from_str(
                    thinking.thinking_signature.as_deref().unwrap_or_default(),
                )
                .expect("signature is JSON");
                assert_eq!(
                    signature,
                    serde_json::json!([
                        {"type": "reasoning.text", "text": "The user wants the time.", "index": 0, "signature": "sha256:text-signature", "format": "openai-responses-v1"},
                        {"type": "reasoning.summary", "summary": "Looked up time.", "index": 0, "format": "openai-responses-v1"},
                        encrypted,
                        later_summary,
                    ])
                );
            }
            other => panic!("expected Done, got {other:?}"),
        }
        apply_all(&events);
    }

    // ---- 11. custom (grammar) tool input ----

    #[tokio::test]
    async fn custom_tool_input_streams_through_grammar_buffer() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}{}",
            data_line(delta_chunk(json!({"tool_calls": [
                {"index": 0, "id": "call_c", "custom": {"name": "write", "input": "abc"}}
            ]}))),
            data_line(delta_chunk(json!({"tool_calls": [
                {"index": 0, "custom": {"input": "def"}}
            ]}))),
            data_line(finish_chunk("tool_calls")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let mut model = base_model();
        model.compat = Some(serde_json::json!({"supportsOpenAIGrammarTools": true}));
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("hi")],
            tools: Some(vec![grammar_tool("write", "text")]),
        });
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;

        assert_eq!(
            event_types(&events),
            [
                "start",
                "toolcall_start",
                "toolcall_delta",
                "toolcall_delta",
                "toolcall_delta",
                "toolcall_end",
                "done"
            ]
        );
        assert_eq!(
            events[2],
            AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: "{\"text\":\"abc".into()
            }
        );
        assert_eq!(
            events[3],
            AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: "def".into()
            }
        );
        assert_eq!(
            events[4],
            AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: "\"}".into()
            }
        );
        match &events[5] {
            AssistantMessageEvent::ToolcallEnd { tool_call, .. } => {
                assert_eq!(tool_call.id, "call_c");
                assert_eq!(tool_call.name, "write");
                assert_eq!(tool_call.arguments, serde_json::json!({"text": "abcdef"}));
            }
            other => panic!("expected ToolcallEnd, got {other:?}"),
        }
        match events.last().unwrap() {
            AssistantMessageEvent::Done { reason, .. } => {
                assert_eq!(*reason, SuccessReason::ToolUse)
            }
            other => panic!("expected Done, got {other:?}"),
        }
        apply_all(&events);
    }

    // ---- 12. streamSimple option mapping ----

    #[tokio::test]
    async fn stream_simple_maps_reasoning_tool_choice_and_default_options() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}",
            data_line(
                serde_json::json!({"id": "chatcmpl-1", "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": "stop"}]})
            ),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let mut model = base_model();
        model.reasoning = true;
        let options = SimpleStreamOptions {
            tool_choice: Some(ToolChoice::None),
            reasoning: Some(crate::ai::types::ThinkingLevel::High),
            ..SimpleStreamOptions::default()
        };
        let events =
            collect_simple(&server, &model, &user_ctx(vec![user_msg("hi")]), &options).await;

        match events.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                assert_eq!(message.stop_reason, StopReason::Stop);
            }
            other => panic!("expected Done, got {other:?}"),
        }

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(sent["model"], "gpt-test");
        assert_eq!(sent["stream"], true);
        assert_eq!(
            sent["stream_options"],
            serde_json::json!({"include_usage": true})
        );
        assert_eq!(sent["reasoning_effort"], "high");
        assert_eq!(sent["tool_choice"], "none");
        assert_eq!(sent["store"], false);
        assert!(sent["max_completion_tokens"].is_u64(), "{sent}");
        assert!(sent.get("max_tokens").is_none(), "{sent}");
        // Authorization header carries the provider key.
        let auth = requests[0]
            .headers
            .get("authorization")
            .map(|value| value.to_str().unwrap().to_string())
            .unwrap_or_default();
        assert_eq!(auth, "Bearer k");
    }

    #[tokio::test]
    async fn stream_applies_stream_options_headers_and_auth() {
        let server = wiremock::MockServer::start().await;
        let body = format!("{}{}", data_line(finish_chunk("stop")), done_line());
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let mut options = StreamOptions {
            temperature: Some(0.5),
            max_tokens: Some(123),
            ..StreamOptions::default()
        };
        let mut headers = BTreeMap::new();
        headers.insert("x-custom".to_string(), Some("v".to_string()));
        options.headers = Some(headers);

        let model = base_model();
        let events =
            collect_stream(&server, &model, &user_ctx(vec![user_msg("hi")]), &options).await;

        assert_eq!(event_types(&events), ["start", "done"]);
        let requests = server.received_requests().await.unwrap();
        let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(sent["temperature"], 0.5);
        assert_eq!(sent["max_completion_tokens"], 123);
        let custom = requests[0]
            .headers
            .get("x-custom")
            .map(|value| value.to_str().unwrap().to_string())
            .unwrap_or_default();
        assert_eq!(custom, "v");
        let auth = requests[0]
            .headers
            .get("authorization")
            .map(|value| value.to_str().unwrap().to_string())
            .unwrap_or_default();
        assert_eq!(auth, "Bearer k");
    }

    // ---- 13. emoji tool results and orphaned tool calls (upstream
    // unicode-surrogate / tool-call-without-result oracles, wire level) ----

    #[tokio::test]
    async fn emoji_tool_results_and_orphaned_tool_calls_stream_clean() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}",
            data_line(content_chunk("sure")),
            data_line(finish_chunk("stop")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let tool_call = crate::ai::types::ToolCall {
            id: "test_1".into(),
            name: "test_tool".into(),
            arguments: serde_json::json!({}),
            thought_signature: None,
            namespace: None,
        };
        // Orphaned tool call: assistant tool call without a matching tool
        // result, followed by a user message (upstream
        // tool-call-without-result.test.ts scenario).
        let assistant = Message::Assistant(crate::ai::types::AssistantMessage {
            content: vec![AssistantBlock::ToolCall(tool_call)],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-test".into(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: Usage {
                cost: UsageCost::default(),
                ..Usage::default()
            },
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: TS,
        });
        // Emoji-bearing tool result content (upstream
        // unicode-surrogate.test.ts scenario).
        let emoji_result = Message::ToolResult(ToolResultMessage {
            tool_call_id: "test_1".into(),
            tool_name: "test_tool".into(),
            content: vec![crate::ai::types::TextOrImageBlock::Text(TextContent {
                text: "Test with emoji \u{1F648} and rocket \u{1F680}".into(),
                text_signature: None,
            })],
            details: None,
            usage: None,
            is_error: false,
            timestamp: TS,
        });
        let ctx = user_ctx(vec![
            user_msg("use it"),
            assistant,
            emoji_result,
            user_msg("summarize"),
        ]);

        let model = base_model();
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "text_end", "done"]
        );
        apply_all(&events);

        let requests = server.received_requests().await.unwrap();
        let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
        // The emoji tool result text reaches the wire verbatim.
        let serialized = sent.to_string();
        assert!(
            serialized.contains('\u{1F648}'),
            "emoji on the wire: {serialized}"
        );
    }

    // ---- 14. request assembly reaches the same builder as T3 ----

    #[tokio::test]
    async fn request_body_matches_build_request() {
        let server = wiremock::MockServer::start().await;
        let body = format!("{}{}", data_line(finish_chunk("stop")), done_line());
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let model = base_model();
        let ctx = user_ctx(vec![user_msg("hi")]);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(event_types(&events), ["start", "done"]);

        let requests = server.received_requests().await.unwrap();
        let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
        let compat = merge_compat(
            detect_openai_completions_compat_for_model(&model.provider, &model.base_url, &model.id),
            model.compat.as_ref(),
        );
        let RequestAssembly { body: expected, .. } = build_request(
            &model,
            &cfg(&server),
            &ctx,
            &SimpleStreamOptions::default(),
            &compat,
        )
        .unwrap();
        assert_eq!(sent, expected, "wire body must equal the T3 builder output");
    }

    // ---- T8: provider-request retry on the send seam ----

    /// First POST answers 429 (with a tiny server-requested delay), later
    /// POSTs stream a complete response.
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
                wiremock::ResponseTemplate::new(429)
                    .insert_header("retry-after-ms", "15")
                    .set_body_string("rate limited")
            } else {
                self.success.clone()
            }
        }
    }

    #[tokio::test]
    async fn send_seam_retries_a_429_before_the_first_stream_byte() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}",
            data_line(content_chunk("ok")),
            data_line(finish_chunk("stop")),
            done_line()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(RetryFlaky {
                attempts: std::sync::atomic::AtomicU32::new(0),
                success: sse(&body),
            })
            .mount(&server)
            .await;

        let model = base_model();
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                max_retries: Some(1),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let events =
            collect_simple(&server, &model, &user_ctx(vec![user_msg("hi")]), &options).await;
        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "text_end", "done"]
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn send_seam_does_not_retry_a_401() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;

        let model = base_model();
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                max_retries: Some(3),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let events =
            collect_simple(&server, &model, &user_ctx(vec![user_msg("hi")]), &options).await;
        assert_eq!(events.len(), 1, "lone error event: {events:?}");
        assert!(matches!(events[0], AssistantMessageEvent::Error { .. }));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    // ---- abort surface ----

    /// A pre-cancelled signal fails the request setup (the retry seam's
    /// `"Request aborted"` abort error) before `Start`, and the catch block
    /// settles `stopReason: "aborted"`.
    #[tokio::test]
    async fn pre_aborted_request_settles_aborted_before_start() {
        let server = wiremock::MockServer::start().await;
        let token = CancellationToken::new();
        token.cancel();
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                signal: Some(token),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        let api = OpenAiCompletions;
        let mut rx = api.stream_simple(
            &cfg(&server),
            &base_model(),
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
    /// stream aborted with `"Request was aborted"` (upstream lines 682-688).
    #[tokio::test]
    async fn mid_stream_cancellation_settles_the_stream_aborted() {
        let base_url = stalled_sse_server().await;
        let cfg = ProviderConfig {
            base_url,
            api_key: "k".to_string(),
            max_tokens: 8192,
        };
        let token = CancellationToken::new();
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                signal: Some(token.clone()),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        let api = OpenAiCompletions;
        let mut rx = api.stream_simple(
            &cfg,
            &base_model(),
            &user_ctx(vec![user_msg("hi")]),
            &options,
        );
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

    #[tokio::test]
    async fn request_callbacks_replace_the_actual_wire_body_and_await_metadata_before_start() {
        use std::sync::Arc;
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_json(
                serde_json::json!({"patched":true}),
            ))
            .respond_with(
                sse(&(data_line(content_chunk("ok"))
                    + &data_line(finish_chunk("stop"))
                    + &done_line()))
                .insert_header("x-callback-test", "observed"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let mut options = StreamOptions::default();
        options.callbacks.on_payload = Some(Arc::new(|payload, model| {
            assert_eq!(model.id, "gpt-test");
            assert_eq!(payload["model"], "gpt-test");
            Box::pin(async { Ok(Some(serde_json::json!({"patched":true}))) })
        }));
        let hook_entered = Arc::clone(&entered);
        let hook_release = Arc::clone(&release);
        options.callbacks.on_response = Some(Arc::new(move |metadata, model| {
            assert_eq!(model.id, "gpt-test");
            assert_eq!(metadata.status, 200);
            assert_eq!(
                metadata.headers.get("x-callback-test").map(String::as_str),
                Some("observed")
            );
            let entered = Arc::clone(&hook_entered);
            let release = Arc::clone(&hook_release);
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                Ok(())
            })
        }));
        let mut stream = OpenAiCompletions.stream(
            &cfg(&server),
            &base_model(),
            &user_ctx(vec![user_msg("hi")]),
            &options,
        );
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        assert!(
            matches!(
                stream.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "Start must wait for onResponse"
        );
        release.notify_one();
        let mut events = Vec::new();
        while let Some(event) = stream.recv().await {
            events.push(event);
        }
        assert_eq!(events.first().unwrap().event_type(), "start");
        assert_eq!(events.last().unwrap().event_type(), "done");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn request_callbacks_none_preserves_payload_but_some_null_replaces_it() {
        use std::sync::Arc;
        for replacement in [None, Some(serde_json::Value::Null)] {
            let server = wiremock::MockServer::start().await;
            wiremock::Mock::given(wiremock::matchers::method("POST"))
                .respond_with(sse(&(data_line(finish_chunk("stop")) + &done_line())))
                .expect(1)
                .mount(&server)
                .await;
            let expected = replacement.clone();
            let mut options = SimpleStreamOptions::default();
            options.stream.callbacks.on_payload = Some(Arc::new(move |_, _| {
                let replacement = replacement.clone();
                Box::pin(async move { Ok(replacement) })
            }));
            let events = collect_simple(&server, &base_model(), &user_ctx(vec![]), &options).await;
            assert_eq!(events.last().unwrap().event_type(), "done");
            let requests = server.received_requests().await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
            match expected {
                None => assert_eq!(body["model"], "gpt-test"),
                Some(_) => assert!(body.is_null()),
            }
        }
    }

    #[tokio::test]
    async fn request_callbacks_payload_failure_is_pre_send_and_response_failure_is_pre_start() {
        use std::sync::Arc;
        for payload_failure in [true, false] {
            let server = wiremock::MockServer::start().await;
            wiremock::Mock::given(wiremock::matchers::method("POST"))
                .respond_with(sse(&(data_line(finish_chunk("stop")) + &done_line())))
                .mount(&server)
                .await;
            let mut options = SimpleStreamOptions::default();
            options.stream.max_retries = Some(2);
            if payload_failure {
                options.stream.callbacks.on_payload = Some(Arc::new(|_, _| {
                    Box::pin(async { anyhow::bail!("payload refused") })
                }));
            } else {
                options.stream.callbacks.on_response = Some(Arc::new(|_, _| {
                    Box::pin(async { anyhow::bail!("response refused") })
                }));
            }
            let events = collect_simple(&server, &base_model(), &user_ctx(vec![]), &options).await;
            assert_eq!(event_types(&events), vec!["error"]);
            let AssistantMessageEvent::Error { error, .. } = &events[0] else {
                panic!("error settlement")
            };
            assert_eq!(
                error.error_message.as_deref(),
                Some(if payload_failure {
                    "payload refused"
                } else {
                    "response refused"
                })
            );
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                usize::from(!payload_failure),
                "hook rejection must not be retried"
            );
        }
    }

    #[tokio::test]
    async fn request_callbacks_non_success_response_never_invokes_success_metadata_hook() {
        use std::sync::Arc;
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("denied"))
            .expect(1)
            .mount(&server)
            .await;
        let mut options = SimpleStreamOptions::default();
        options.stream.callbacks.on_response = Some(Arc::new(|_, _| {
            panic!("SDK failure must precede onResponse")
        }));
        let events = collect_simple(&server, &base_model(), &user_ctx(vec![]), &options).await;
        assert_eq!(event_types(&events), vec!["error"]);
    }
}
