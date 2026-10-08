//! OpenAI Codex Responses API — full port of upstream
//! `packages/ai/src/api/openai-codex-responses.ts` (1666 lines): the ChatGPT
//! backend `/codex/responses` SSE endpoint with its own retry policy, zstd
//! request compression, JWT account-id extraction, the ChatGPT header shape,
//! and the websocket transport (default `auto` = websocket-first with SSE
//! fallback) including the websocket-cached continuation protocol, in
//! [`websocket`].
//!
//! Function mapping (upstream line -> port):
//! - `stream` (237-498) -> [`run_stream_task`]
//! - `streamSimple` (500-521) -> [`OpenAiCodexResponses::stream_simple`]
//! - `buildRequestBody` (527-600) -> [`build_request_body`]
//! - `resolveCodexUrl` / `resolveCodexWebSocketUrl` (641-654) -> [`resolve_codex_url`] / [`resolve_codex_websocket_url`]
//! - service-tier pricing (602-639) -> [`get_service_tier_cost_multiplier`] / [`apply_service_tier_pricing`] / [`resolve_codex_service_tier`]
//! - `mapCodexEvents` / `normalizeCodexStatus` / `extractCodexEventError` (700-767) -> [`map_codex_event`]
//! - `parseSSE` (773-833) -> [`process_sse_stream`]
//! - `parseErrorResponse` (1565-1590) -> [`parse_error_response`]
//! - `extractAccountId` (1596-1607) -> [`extract_account_id`]
//! - header builders (1609-1666) -> [`build_base_codex_headers`] / [`build_sse_headers`] / [`build_websocket_headers`]
//! - retry helpers (123-198) -> [`is_retryable_error`] / [`get_retry_after_delay_ms`] / [`validate_retry_delay_ms`]
//! - `compressRequestBodyZstd` (208-231) -> [`compress_request_body_zstd`]
//! - `uuidv7` (`utils/uuid.ts`) -> [`uuid_v7`]
//!
//! Deviations from upstream, all structural:
//! - OAuth token acquisition/refresh is M2d (controller ruling): the bearer
//!   JWT arrives via `options.apiKey` (falling back to the provider config
//!   key), and the auth *shape* — `Authorization: Bearer`, the
//!   `chatgpt-account-id` header extracted from the JWT claim
//!   `https://api.openai.com/auth` — ports now.
//! - The websocket transport is load-bearing for the default flow (upstream
//!   `options?.transport || "auto"`, line 294; the coding-agent default
//!   setting is `"auto"`), so it is fully ported, including session-cached
//!   sockets and the cached-continuation protocol. The runtime `WebSocket`
//!   global becomes an injectable connector; production uses
//!   tokio-tungstenite (rustls, matching reqwest's TLS stack). The bun proxy
//!   constructor branch (lines 971-994) has no equivalent.
//! - `options.signal` is the port's non-serialized
//!   [`CancellationToken`](tokio_util::sync::CancellationToken) (upstream
//!   `AbortSignal`): it aborts the websocket dial and message reads, the SSE
//!   header wait and body reads, the retry backoff sleeps, and settles the
//!   catch block's `"aborted"` branch with `"Request was aborted"`
//!   (upstream lines 330-340, 385-461, 773-833, 484-494). `fetch` injection
//!   has no port surface, and `streamSimple`'s synchronous missing-key
//!   throw surfaces as the async error event (port contract, same as the
//!   sibling ports). The hooks are wired (M2 correction): `onPayload`
//!   (openai-codex-responses.ts:278) is awaited before the transport
//!   selection — the replacement feeds both the websocket frame and the SSE
//!   body — and `onResponse` (line 414) fires per SSE attempt after the fetch
//!   resolves and before the ok check (non-success responses included; the
//!   websocket transport has no upstream call site). A response-hook
//!   rejection becomes the attempt error and retries like any other
//!   non-"usage limit" attempt error before surfacing.
//! - The `OpenAICodexResponsesOptions` extensions have no public option
//!   surface: `reasoningEffort`/`reasoningSummary`/`serviceTier`/
//!   `textVerbosity`/direct-stream `toolChoice` are carried on the internal
//!   [`CodexRequestOptions`] (defaulted by the public entry points), so the
//!   pure builder and the wire stay testable; `reasoningSummary` always
//!   renders `"auto"` (the `?? "auto"` default — upstream `null` also falls
//!   back to it).
//! - `options.signal` doubling as the websocket idle timeout is preserved
//!   via `timeoutMs`; the header-timeout error message is reproduced
//!   byte-for-byte, but a timeout DURING the SSE body surfaces with
//!   reqwest's message text instead of the DOMException text upstream
//!   produces (the shared-deadline behavior matches).
//! - The catch block's scratch cleanup (`delete partialJson`, line 486-489)
//!   is a no-op here: the shared processor keeps streaming scratch in
//!   internal slots that never persist into content blocks.
//! - HTTP error bodies go through [`parse_error_response`] (codex-specific
//!   ChatGPT usage-limit rendering); `formatProviderError(
//!   normalizeProviderError(error))` in the outer catch reduces to the error
//!   message itself because codex errors carry no SDK status/body fields.
//! - Retry here is NOT the pinned SDK policy of [`crate::ai::retry`]:
//!   upstream codex ships its own loop (status set 429/500/502/503/504 plus
//!   text patterns, terminal quota errors fail fast, jitter-free
//!   `1000 * 2^attempt` backoff, `retry-after`/`retry-after-ms` honoring
//!   including HTTP-date form, and a catch-all path that also retries
//!   non-retryable statuses while attempts remain). Ported verbatim; the
//!   pattern matcher is shared with `crate::ai::retry`.
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order; `requestBodiesMatchExceptInput` equality therefore compares
//!   deterministic serializations (same documented deviation as the request
//!   builders).
//! - `x-request-id`-style SDK telemetry headers are not sent; `zstd`
//!   compression is always available (the browser fallback to an
//!   uncompressed body, line 215-217, is unreachable).
//! - A failure reading the ERROR response body (`response.text()`, line 423)
//!   is treated as an empty body here, while upstream would throw into the
//!   catch block and retry it as a network error.
//! - The websocket `readyState` upstream reads for reuse decisions is
//!   modeled as an explicit health byte on [`websocket::WsConnection`]; the
//!   socket-idle close reason and the age-limit close reason match upstream.

#[cfg(test)]
mod callback_tests;

pub mod websocket;

use std::collections::HashSet;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::ai::api::openai_completions::request::{
    clamp_max_tokens_to_context, clamp_openai_prompt_cache_key, clamp_thinking_level,
    create_grammar_tool_input_properties, level_key, map_level, MappedLevel,
};
use crate::ai::api::openai_responses_shared::{
    convert_responses_messages, convert_responses_tools, ConvertResponsesMessagesOptions,
    ConvertResponsesToolsOptions, ResponsesResponse, ResponsesStreamEvent, ResponsesStreamOptions,
    ResponsesStreamProcessor,
};
use crate::ai::api::{http_client, pi_user_agent, request_signal, ApiImpl, REQUEST_WAS_ABORTED};
use crate::ai::retry::pattern_matches;
use crate::ai::transcript::{
    get_declared_tools, get_initial_system_message, get_system_message_text, resolve_transcript,
    resolve_transcript_tools, TranscriptContext,
};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::message::{
    AssistantMessage, AssistantMessageDiagnostic, DiagnosticCode, DiagnosticErrorInfo,
};
use crate::ai::types::options::{SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{CacheRetention, StopReason, ToolChoice, Transport, Usage};
use crate::ai::types::Model;
use crate::ai::uuid::uuid_v7;
use crate::ai::{now_ms, ProviderConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Upstream `DEFAULT_CODEX_BASE_URL` (line 52).
const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
/// Upstream `JWT_CLAIM_PATH` (line 53). `pub(crate)`: the oauth flow port
/// (`auth::oauth::openai_codex`) reads the same claim when it extracts the
/// ChatGPT account id from the access token.
pub(crate) const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";
/// Upstream `BASE_DELAY_MS` (line 55).
const BASE_DELAY_MS: u64 = 1000;
/// Upstream `DEFAULT_MAX_RETRY_DELAY_MS` (line 56).
const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;
/// Upstream `REQUEST_COMPRESSION_ZSTD_LEVEL` (line 60).
const REQUEST_COMPRESSION_ZSTD_LEVEL: i32 = 3;
/// Upstream `CODEX_TOOL_CALL_PROVIDERS` (line 61): providers whose tool calls
/// keep their `call_id|item_id` wire ids on replay.
pub(crate) const CODEX_TOOL_CALL_PROVIDERS: [&str; 3] = ["openai", "openai-codex", "opencode"];
/// Upstream `OPENAI_BETA_RESPONSES_WEBSOCKETS` (line 839).
const OPENAI_BETA_RESPONSES_WEBSOCKETS: &str = "responses_websockets=2026-02-06";
/// Upstream `PREVIOUS_RESPONSE_NOT_FOUND_CODE` (line 64).
const PREVIOUS_RESPONSE_NOT_FOUND_CODE: &str = "previous_response_not_found";
/// Upstream `WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE` (line 63).
const WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE: &str = "websocket_connection_limit_reached";
/// Upstream `instructions` fallback (line 557).
const DEFAULT_INSTRUCTIONS: &str = "You are a helpful assistant.";

/// Upstream `CODEX_RESPONSE_STATUSES` (lines 66-73): statuses the codex
/// backend may report; anything else is normalized away from the terminal
/// event.
const CODEX_RESPONSE_STATUSES: [&str; 6] = [
    "completed",
    "incomplete",
    "failed",
    "cancelled",
    "queued",
    "in_progress",
];

// =============================================================================
// Options and errors
// =============================================================================

/// Upstream `OpenAICodexResponsesOptions` extension fields (lines 79-85)
/// minus the base `StreamOptions` (carried by [`SimpleStreamOptions`]).
/// Internal: the public entry points default these the way the only reachable
/// upstream flows do (`streamSimple` forwards `toolChoice` and the clamped
/// reasoning level; direct `stream` callers pass nothing).
#[derive(Debug, Clone, Default)]
pub(crate) struct CodexRequestOptions {
    /// Upstream `reasoningEffort` (the string form, pre-mapping).
    pub reasoning_effort: Option<String>,
    /// Upstream `serviceTier`.
    pub service_tier: Option<String>,
    /// Upstream `textVerbosity`.
    pub text_verbosity: Option<String>,
    /// Upstream `toolChoice` (`"auto" | "none" | "required"`).
    pub tool_choice: Option<String>,
}

/// The codex-specific error taxonomy (upstream `CodexApiError` /
/// `CodexProtocolError` / plain errors / `WebSocketCloseError` /
/// `RetryDelayExceededError`). The kind drives transport-fallback decisions
/// (`isCodexNonTransportError`) and diagnostic metadata (`error.name`).
#[derive(Debug, Clone)]
pub(crate) struct CodexStreamError {
    pub kind: CodexStreamErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexStreamErrorKind {
    /// `CodexApiError`: a codex `error` event or `response.failed`.
    Api { code: Option<String> },
    /// `CodexProtocolError`: malformed stream frames.
    Protocol,
    /// `WebSocketCloseError`.
    Close {
        code: Option<u16>,
        reason: Option<String>,
    },
    /// Any other thrown value.
    Plain,
    /// `RetryDelayExceededError`: never retried.
    RetryDelayExceeded,
}

impl CodexStreamError {
    pub(crate) fn plain(message: impl Into<String>) -> Self {
        CodexStreamError {
            kind: CodexStreamErrorKind::Plain,
            message: message.into(),
        }
    }

    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        CodexStreamError {
            kind: CodexStreamErrorKind::Protocol,
            message: message.into(),
        }
    }

    pub(crate) fn api(code: Option<String>, message: impl Into<String>) -> Self {
        CodexStreamError {
            kind: CodexStreamErrorKind::Api { code },
            message: message.into(),
        }
    }

    pub(crate) fn close(code: Option<u16>, reason: Option<String>, message: String) -> Self {
        CodexStreamError {
            kind: CodexStreamErrorKind::Close { code, reason },
            message,
        }
    }

    fn retry_delay_exceeded(message: String) -> Self {
        CodexStreamError {
            kind: CodexStreamErrorKind::RetryDelayExceeded,
            message,
        }
    }

    /// Upstream `error.name` for the transport-failure diagnostic.
    pub(crate) fn name(&self) -> &'static str {
        match self.kind {
            CodexStreamErrorKind::Api { .. } => "CodexApiError",
            CodexStreamErrorKind::Protocol => "CodexProtocolError",
            CodexStreamErrorKind::Close { .. } => "WebSocketCloseError",
            CodexStreamErrorKind::Plain | CodexStreamErrorKind::RetryDelayExceeded => "Error",
        }
    }

    /// Upstream `isCodexNonTransportError` (lines 700-702).
    pub(crate) fn is_non_transport(&self) -> bool {
        matches!(
            self.kind,
            CodexStreamErrorKind::Api { .. } | CodexStreamErrorKind::Protocol
        )
    }

    /// Upstream `isWebSocketConnectionLimitReachedError` (lines 704-706).
    pub(crate) fn is_connection_limit(&self) -> bool {
        matches!(
            &self.kind,
            CodexStreamErrorKind::Api { code }
                if code.as_deref() == Some(WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE)
        )
    }

    /// Upstream `isPreviousResponseNotFoundError` (lines 708-710).
    pub(crate) fn is_previous_response_not_found(&self) -> bool {
        matches!(
            &self.kind,
            CodexStreamErrorKind::Api { code }
                if code.as_deref() == Some(PREVIOUS_RESPONSE_NOT_FOUND_CODE)
        )
    }

    /// The `DiagnosticErrorInfo.code` field (`error.code` when set).
    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        match &self.kind {
            CodexStreamErrorKind::Api { code: Some(code) } => {
                Some(DiagnosticCode::String(code.clone()))
            }
            CodexStreamErrorKind::Close { code, .. } => {
                code.map(|code| DiagnosticCode::Number(serde_json::Number::from(code)))
            }
            _ => None,
        }
    }
}

impl From<String> for CodexStreamError {
    fn from(message: String) -> Self {
        CodexStreamError::plain(message)
    }
}

// =============================================================================
// Entry points
// =============================================================================

pub struct OpenAiCodexResponses;

impl ApiImpl for OpenAiCodexResponses {
    // Upstream wires both `options?.onPayload?.(body, model)`
    // (openai-codex-responses.ts:278, before the transport selection so the
    // replacement feeds both transports) and onResponse (line 414, per SSE
    // attempt before the ok check; the websocket transport has no call site).
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
        // Upstream direct-`stream` callers pass the API-specific extensions;
        // the port's `StreamOptions` surface carries none of them, so the
        // extension fields stay unset (module docs).
        let simple = SimpleStreamOptions {
            stream: options.clone(),
            ..SimpleStreamOptions::default()
        };
        run_stream(
            cfg.clone(),
            model.clone(),
            ctx.clone(),
            simple,
            CodexRequestOptions::default(),
        )
    }

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream `streamSimple` (lines 500-521): buildBaseOptions shaping
        // (context-clamped maxTokens default), the reasoning clamp ("off"
        // drops the effort entirely), and the `toolChoice` passthrough. The
        // synchronous missing-key throw surfaces as the async error event
        // (port contract).
        let mut shaped = options.clone();
        shaped.stream.max_tokens = Some(clamp_max_tokens_to_context(
            model,
            ctx,
            options.stream.max_tokens.unwrap_or(model.max_tokens),
        ));
        shaped.reasoning = options
            .reasoning
            .and_then(|level| clamp_thinking_level(model, Some(level)));
        // Upstream lines 514-515: the clamped reasoning becomes the
        // string effort; a clamped "off" drops it entirely (the port's
        // clamp returns None for "off").
        let codex = CodexRequestOptions {
            reasoning_effort: shaped
                .reasoning
                .map(|level| level_key(Some(level)).to_string()),
            tool_choice: options.tool_choice.map(|choice| match choice {
                ToolChoice::Auto => "auto".to_string(),
                ToolChoice::None => "none".to_string(),
            }),
            ..CodexRequestOptions::default()
        };
        run_stream(cfg.clone(), model.clone(), ctx.clone(), shaped, codex)
    }
}

/// Test seam mirroring upstream direct-`stream` calls that pass the API
/// option extensions (`reasoningEffort`/`serviceTier`/`toolChoice`/...).
#[cfg(test)]
pub(crate) fn stream_with_codex_options(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: SimpleStreamOptions,
    codex: CodexRequestOptions,
) -> mpsc::Receiver<AssistantMessageEvent> {
    run_stream(cfg, model, ctx, options, codex)
}

fn run_stream(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: SimpleStreamOptions,
    codex: CodexRequestOptions,
) -> mpsc::Receiver<AssistantMessageEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        run_stream_task(cfg, model, ctx, options, codex, tx).await;
    });
    rx
}

async fn run_stream_task(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: SimpleStreamOptions,
    codex: CodexRequestOptions,
    tx: mpsc::Sender<AssistantMessageEvent>,
) {
    // Upstream line 243: resolve the transcript synchronously before
    // anything else, then compute grammar properties from the NORMALIZED
    // messages (lines 271-274). Both are infallible here; the apiKey and
    // grammar checks keep their upstream precedence inside the flow below.
    let supports_mid_convo = model
        .openai_responses_compat()
        .ok()
        .and_then(|compat| compat.supports_mid_convo_system_messages)
        .unwrap_or(false);
    let normalized = resolve_transcript(ctx, Some(supports_mid_convo));

    let grammar_result = create_grammar_tool_input_properties(
        &get_declared_tools(normalized.messages()),
        model
            .openai_responses_compat()
            .ok()
            .and_then(|compat| compat.supports_openai_grammar_tools)
            .unwrap_or(false),
    );

    // The model id (for the gpt-5.5 priority multiplier) moves into the
    // pricing hook.
    let model_id = model.id.clone();
    let mut processor = ResponsesStreamProcessor::new(
        &model,
        ResponsesStreamOptions {
            callbacks: options.stream.callbacks.clone(),
            service_tier: codex.service_tier.clone(),
            grammar_tool_input_properties: grammar_result.clone().unwrap_or_default(),
            resolve_service_tier: Some(Box::new(resolve_codex_service_tier)),
            apply_service_tier_pricing: Some(Box::new(move |usage, tier| {
                apply_service_tier_pricing(usage, tier.as_deref(), &model_id);
            })),
        },
    );

    let signal = request_signal(&options.stream.signal);
    let outcome: Result<(), CodexStreamError> = async {
        // Upstream lines 265-268: `options?.apiKey` (the port adds the
        // provider-config fallback per the M2c controller ruling).
        let api_key = options
            .stream
            .api_key
            .clone()
            .or_else(|| Some(cfg.api_key.clone()))
            .filter(|key| !key.is_empty())
            .ok_or_else(|| format!("No API key for provider: {}", model.provider))?;

        // Upstream line 270: the JWT account id precedes body building.
        let account_id = extract_account_id(&api_key)?;

        // Upstream lines 271-274: grammar properties (after apiKey).
        let grammar_tool_input_properties = grammar_result?;

        // Upstream line 275-276: cache retention gates the session id, and
        // the codex session id is the OpenAI 64-char clamp.
        let cache_session_id: Option<String> =
            if options.stream.cache_retention == Some(CacheRetention::None) {
                None
            } else {
                options.stream.session_id.clone()
            };
        let codex_session_id = clamp_openai_prompt_cache_key(cache_session_id.as_deref());

        // Upstream lines 277-291: request body, websocket request id, both
        // header sets, the JSON body.
        let body = build_request_body(
            &model,
            &normalized,
            &options,
            &codex,
            codex_session_id.as_deref(),
            &grammar_tool_input_properties,
        )
        .map_err(CodexStreamError::plain)?;
        // Upstream lines 278-281: onPayload is awaited before the transport
        // selection, so the replacement feeds BOTH the websocket frame and
        // the SSE body; JS `undefined` preserves the built body while any
        // other value — including `null` — replaces it. A hook rejection
        // throws into the catch block before any transport traffic.
        let body = options
            .stream
            .callbacks
            .payload(body, &model)
            .await
            .map_err(|error| CodexStreamError::plain(error.to_string()))?;
        let websocket_request_id = codex_session_id.clone().unwrap_or_else(uuid_v7);
        let mut sse_headers = build_sse_headers(
            &model,
            &options,
            &account_id,
            &api_key,
            codex_session_id.as_deref(),
        );
        let websocket_headers = build_websocket_headers(
            &model,
            &options,
            &account_id,
            &api_key,
            &websocket_request_id,
        );
        let body_json = serde_json::to_string(&body)
            .map_err(|error| CodexStreamError::plain(error.to_string()))?;
        let http_timeout_ms = options.stream.timeout_ms;
        let websocket_connect_timeout_ms = options.stream.websocket_connect_timeout_ms;
        let transport = options.stream.transport.unwrap_or(Transport::Auto);
        let mut start_emitted = false;

        // Upstream lines 296-299: sessions that already fell back skip the
        // websocket attempt entirely.
        let websocket_disabled_for_session = transport != Transport::Sse
            && websocket::is_websocket_sse_fallback_active(cache_session_id.as_deref());
        if websocket_disabled_for_session {
            websocket::record_websocket_sse_fallback(cache_session_id.as_deref());
        }

        // Upstream lines 301-374: the websocket attempt loop.
        if transport != Transport::Sse && !websocket_disabled_for_session {
            let mut retried_connection_limit = false;
            let mut retried_missing_continuation = false;
            loop {
                let mut websocket_started = false;
                let outcome = websocket::process_websocket_stream(websocket::WsAttempt {
                    url: resolve_codex_websocket_url(Some(&model.base_url)),
                    body: &body,
                    ws_headers: &websocket_headers,
                    model: &model,
                    processor: &mut processor,
                    tx: &tx,
                    start_emitted: &mut start_emitted,
                    attempt_started: &mut websocket_started,
                    idle_timeout_ms: http_timeout_ms,
                    connect_timeout_ms: websocket_connect_timeout_ms,
                    cache_session_id: cache_session_id.as_deref(),
                    account_id: &account_id,
                    use_cached_context: matches!(
                        transport,
                        Transport::WebsocketCached | Transport::Auto
                    ),
                    grammar_tool_input_properties: &grammar_tool_input_properties,
                    signal: signal.clone(),
                })
                .await;

                // Upstream reads `options?.signal?.aborted` in the catch and
                // after success: an aborted request skips both retry arms and
                // the post-success guard throws the abort error.
                let aborted = signal.is_cancelled();
                match outcome {
                    Ok(end_turn) => {
                        if let Some(end_turn) = end_turn {
                            processor.output_mut().end_turn = Some(end_turn);
                        }
                        if aborted {
                            return Err(CodexStreamError::plain(REQUEST_WAS_ABORTED));
                        }
                        // Upstream lines 330-340: post-success guard, done,
                        // end.
                        assert_successful_output(processor.output())
                            .map_err(CodexStreamError::plain)?;
                        let _ = tx
                            .send(AssistantMessageEvent::Done {
                                reason: success_reason(processor.output().stop_reason),
                                message: processor.output().clone(),
                            })
                            .await;
                        return Ok(());
                    }
                    Err(error) => {
                        let connection_limit_before_start =
                            !websocket_started && error.is_connection_limit();
                        let previous_not_found = error.is_previous_response_not_found();
                        if !aborted && previous_not_found && !retried_missing_continuation {
                            retried_missing_continuation = true;
                            continue;
                        }
                        if !aborted && connection_limit_before_start && !retried_connection_limit {
                            retried_connection_limit = true;
                            continue;
                        }
                        if aborted || (error.is_non_transport() && !connection_limit_before_start) {
                            return Err(error);
                        }
                        append_transport_failure_diagnostic(
                            processor.output_mut(),
                            &error,
                            transport,
                            websocket_started,
                            &body_json,
                        );
                        websocket::record_websocket_failure(cache_session_id.as_deref(), &error);
                        if websocket_started {
                            return Err(error);
                        }
                        websocket::record_websocket_sse_fallback(cache_session_id.as_deref());
                        break;
                    }
                }
            }
        }

        // Upstream lines 376-383: compress the request body once for the SSE
        // path (the websocket transport sent the uncompressed JSON frame).
        let compressed = compress_request_body_zstd(&body_json);
        if compressed.is_some() {
            set_header(&mut sse_headers, "content-encoding", "zstd");
        }
        let sse_body = match compressed {
            Some(bytes) => SseBody::Bytes(bytes),
            None => SseBody::Text(body_json),
        };

        // Upstream lines 385-461: the SSE retry loop.
        let max_retries = options.stream.max_retries.unwrap_or(0);
        let mut response: Option<reqwest::Response> = None;
        for attempt in 0..=max_retries {
            // Upstream line 397: a signal aborted before the attempt throws
            // before the fetch.
            if signal.is_cancelled() {
                return Err(CodexStreamError::plain(REQUEST_WAS_ABORTED));
            }
            let thrown: CodexStreamError = match send_sse_request(
                &resolve_codex_url(Some(&model.base_url)),
                &sse_headers,
                &sse_body,
                http_timeout_ms,
                &signal,
            )
            .await
            {
                Ok(received) => {
                    // Upstream lines 414-417: onResponse fires per attempt,
                    // after the fetch resolves and before the ok check (so
                    // it also observes the non-success responses that drive
                    // the retry decisions); a hook rejection becomes the
                    // attempt error and rides the catch block below, exactly
                    // like any other thrown error.
                    let hook = options
                        .stream
                        .callbacks
                        .http_response(&received, &model)
                        .await;
                    if let Err(error) = hook {
                        CodexStreamError::plain(error.to_string())
                    } else if received.status().is_success() {
                        response = Some(received);
                        break;
                    } else {
                        let status = received.status().as_u16();
                        let response_headers = received.headers().clone();
                        let error_text = received.text().await.unwrap_or_default();
                        if attempt < max_retries && is_retryable_error(status, &error_text) {
                            let delay = match get_retry_after_delay_ms(
                                &response_headers,
                                now_ms() as u64,
                            ) {
                                Some(delay) => validate_retry_delay_ms(
                                    delay,
                                    options.stream.max_retry_delay_ms,
                                )?,
                                None => base_delay_ms(attempt),
                            };
                            sleep_or_abort(delay, &signal).await;
                            continue;
                        }
                        // Final attempt or non-retryable: the friendly/message
                        // error is thrown into the catch block below.
                        let info = parse_error_response(status, &error_text);
                        CodexStreamError::plain(info.friendly_message.unwrap_or(info.message))
                    }
                }
                Err(error) => error,
            };

            // Upstream catch block (lines 442-459): an abort thrown by the
            // fetch ("Request was aborted") rethrows without retry;
            // RetryDelayExceeded rethrows; network-flavored and non-"usage
            // limit" errors retry with the plain exponential backoff (this
            // path also retries non-retryable statuses while attempts remain
            // — upstream behavior). The backoff sleeps abort with the signal.
            if thrown.message == REQUEST_WAS_ABORTED {
                return Err(thrown);
            }
            if matches!(thrown.kind, CodexStreamErrorKind::RetryDelayExceeded) {
                return Err(thrown);
            }
            if attempt < max_retries && !thrown.message.contains("usage limit") {
                let delay = base_delay_ms(attempt);
                sleep_or_abort(delay, &signal).await;
                continue;
            }
            return Err(thrown);
        }
        // Upstream `if (!response?.ok) throw lastError ?? "Failed after
        // retries"` — unreachable in the port (every loop exit either breaks
        // with a response or returns an error), kept for fidelity.
        let response = match response {
            Some(response) => response,
            None => return Err(CodexStreamError::plain("Failed after retries")),
        };

        // Upstream lines 471-475: start (once), then the SSE stream.
        if !start_emitted {
            let _ = tx
                .send(AssistantMessageEvent::Start {
                    message: processor.output().clone(),
                })
                .await;
        }
        let end_turn = process_sse_stream(response, &mut processor, &tx, &signal).await?;
        if let Some(end_turn) = end_turn {
            processor.output_mut().end_turn = Some(end_turn);
        }

        // Upstream post-loop guard inside processResponsesStream, then the
        // success guard (lines 481) and done (line 482).
        processor.finish().map_err(CodexStreamError::plain)?;
        // Upstream line 481's caller: `signal?.aborted` settles the abort
        // error before the success guard.
        if signal.is_cancelled() {
            return Err(CodexStreamError::plain(REQUEST_WAS_ABORTED));
        }
        assert_successful_output(processor.output()).map_err(CodexStreamError::plain)?;
        let _ = tx
            .send(AssistantMessageEvent::Done {
                reason: success_reason(processor.output().stop_reason),
                message: processor.output().clone(),
            })
            .await;
        Ok(())
    }
    .await;

    match outcome {
        Ok(()) => {}
        Err(error) => {
            // Upstream catch block (lines 484-494): the scratch cleanup is a
            // no-op in the port (module docs); stopReason settles to
            // "aborted" when the request signal fired, else "error", and the
            // thrown value's message becomes errorMessage.
            let mut output = processor.into_output();
            output.stop_reason = if signal.is_cancelled() {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            output.error_message = Some(error.message);
            let _ = tx
                .send(AssistantMessageEvent::Error {
                    reason: if signal.is_cancelled() {
                        ErrorReason::Aborted
                    } else {
                        ErrorReason::Error
                    },
                    error: output,
                })
                .await;
        }
    }
}

/// Upstream `assertSuccessfulOutput` (lines 110-117).
fn assert_successful_output(output: &AssistantMessage) -> Result<StopReason, String> {
    match output.stop_reason {
        StopReason::Pending => Err("Codex stream ended without a stop reason".to_string()),
        StopReason::Error | StopReason::Aborted => Err(output
            .error_message
            .clone()
            .unwrap_or_else(|| "An unknown error occurred".to_string())),
        other => Ok(other),
    }
}

/// The `done` reason mapping used by the sibling ports.
fn success_reason(stop_reason: StopReason) -> SuccessReason {
    match stop_reason {
        StopReason::Length => SuccessReason::Length,
        StopReason::ToolUse => SuccessReason::ToolUse,
        _ => SuccessReason::Stop,
    }
}

/// Upstream lines 356-365: the `provider_transport_failure` diagnostic
/// appended to the live output before a websocket failure falls back or
/// rethrows.
fn append_transport_failure_diagnostic(
    output: &mut AssistantMessage,
    error: &CodexStreamError,
    transport: Transport,
    websocket_started: bool,
    body_json: &str,
) {
    let mut details = Map::new();
    details.insert(
        "configuredTransport".into(),
        json!(transport_label(transport)),
    );
    if !websocket_started {
        details.insert("fallbackTransport".into(), json!("sse"));
    }
    details.insert("eventsEmitted".into(), json!(websocket_started));
    details.insert(
        "phase".into(),
        json!(if websocket_started {
            "after_message_stream_start"
        } else {
            "before_message_stream_start"
        }),
    );
    details.insert("requestBytes".into(), json!(body_json.len()));
    output
        .diagnostics
        .get_or_insert_with(Vec::new)
        .push(AssistantMessageDiagnostic {
            r#type: "provider_transport_failure".to_string(),
            timestamp: now_ms(),
            error: Some(DiagnosticErrorInfo {
                name: Some(error.name().to_string()),
                message: error.message.clone(),
                stack: None,
                code: error.diagnostic_code(),
            }),
            details: Some(Value::Object(details)),
        });
}

fn transport_label(transport: Transport) -> &'static str {
    match transport {
        Transport::Sse => "sse",
        Transport::Websocket => "websocket",
        Transport::WebsocketCached => "websocket-cached",
        Transport::Auto => "auto",
    }
}

// =============================================================================
// SSE request + stream (upstream lines 376-475, 773-833)
// =============================================================================

/// The encoded SSE request body (compressed or raw JSON).
enum SseBody {
    Bytes(Vec<u8>),
    Text(String),
}

/// Sends one SSE attempt (upstream lines 396-418 minus the onResponse hook).
/// A timeout during the response-head phase reproduces the upstream message;
/// transport failures carry reqwest's text (module docs). The request signal
/// (`combineAbortSignals([options?.signal, headerTimeoutSignal])` upstream)
/// aborts the wait with the abort error message.
async fn send_sse_request(
    url: &str,
    headers: &[(String, String)],
    body: &SseBody,
    timeout_ms: Option<u64>,
    signal: &CancellationToken,
) -> Result<reqwest::Response, CodexStreamError> {
    let mut header_map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            CodexStreamError::plain(format!("Invalid header name \"{name}\": {error}"))
        })?;
        let value = reqwest::header::HeaderValue::from_str(value).map_err(|error| {
            CodexStreamError::plain(format!("Invalid header value for \"{name}\": {error}"))
        })?;
        header_map.insert(name, value);
    }
    let mut request = http_client().post(url).headers(header_map);
    request = match body {
        SseBody::Bytes(bytes) => request.body(bytes.clone()),
        SseBody::Text(text) => request.body(text.clone()),
    };
    if let Some(ms) = timeout_ms.filter(|ms| *ms > 0) {
        request = request.timeout(Duration::from_millis(ms));
    }
    let future = request.send();
    let result = tokio::select! {
        biased;
        _ = signal.cancelled() => Err(CodexStreamError::plain(REQUEST_WAS_ABORTED)),
        result = future => result.map_err(|error| {
            if error.is_timeout() {
                CodexStreamError::plain(format!(
                    "Codex SSE response headers timed out after {}ms",
                    timeout_ms.unwrap_or(0)
                ))
            } else {
                CodexStreamError::plain(error.to_string())
            }
        }),
    };
    result
}

/// Upstream `sleep(delayMs, options?.signal)` (lines 178-196): the backoff
/// sleep aborts when the signal fires. Cancellation here is picked up by the
/// loop's top-of-attempt check (or the catch's abort rethrow), so this only
/// shortens the wait.
async fn sleep_or_abort(delay_ms: u64, signal: &CancellationToken) {
    tokio::select! {
        biased;
        _ = signal.cancelled() => {}
        _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
    }
}

/// Upstream `processStream` + `parseSSE` (lines 660-674, 773-833): frame the
/// body on blank lines, join `data:` lines, skip `[DONE]`, map codex events
/// onto the shared processor, and stop at the terminal event. Returns the
/// terminal event's `end_turn` value when present. The signal breaks body
/// reads on abort (`parseSSE`'s `signal?.aborted` checks).
async fn process_sse_stream(
    response: reqwest::Response,
    processor: &mut ResponsesStreamProcessor,
    tx: &mpsc::Sender<AssistantMessageEvent>,
    signal: &CancellationToken,
) -> Result<Option<bool>, CodexStreamError> {
    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut end_turn: Option<bool> = None;
    loop {
        if let Some(index) = find_sse_frame_end(&buffer) {
            let frame: Vec<u8> = buffer.drain(..index).collect();
            if process_sse_frame(&frame, processor, tx, &mut end_turn).await? {
                return Ok(end_turn);
            }
            continue;
        }
        let chunk = tokio::select! {
            biased;
            _ = signal.cancelled() => {
                return Err(CodexStreamError::plain(REQUEST_WAS_ABORTED));
            }
            next = stream.next() => match next {
                Some(Ok(chunk)) => chunk,
                Some(Err(error)) => return Err(CodexStreamError::plain(error.to_string())),
                None => {
                    // Upstream lines 794-795: EOF terminates the residual
                    // frame.
                    if buffer.iter().any(|byte| !byte.is_ascii_whitespace()) {
                        buffer.extend_from_slice(b"\n\n");
                        continue;
                    }
                    return Ok(end_turn);
                }
            },
        };
        buffer.extend_from_slice(&chunk);
    }
}

/// Length of the next `\n\n`-delimited frame including the delimiter.
fn find_sse_frame_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| index + 2)
}

/// Parses one frame (upstream lines 798-818). Returns whether the mapped
/// terminal event was processed.
async fn process_sse_frame(
    frame: &[u8],
    processor: &mut ResponsesStreamProcessor,
    tx: &mpsc::Sender<AssistantMessageEvent>,
    end_turn: &mut Option<bool>,
) -> Result<bool, CodexStreamError> {
    let text = String::from_utf8_lossy(frame);
    let data_lines: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("data:"))
        .map(|line| line["data:".len()..].trim())
        .collect();
    if data_lines.is_empty() {
        return Ok(false);
    }
    let data = data_lines.join("\n").trim().to_string();
    if data.is_empty() || data == "[DONE]" {
        return Ok(false);
    }
    let parsed: Value = serde_json::from_str(&data)
        .map_err(|error| CodexStreamError::protocol(format!("Invalid Codex SSE JSON: {error}")))?;
    match map_codex_event(&parsed, end_turn) {
        CodexMapped::Skip => Ok(false),
        CodexMapped::Error(error) => Err(error),
        CodexMapped::Event(event) => {
            processor.observe_raw_event(parsed).await?;
            processor.process_event(&event, tx).await?;
            Ok(false)
        }
        CodexMapped::Terminal(event) => {
            processor.observe_raw_event(parsed).await?;
            processor.process_event(&event, tx).await?;
            Ok(true)
        }
    }
}

// =============================================================================
// Codex event mapping (upstream lines 712-767)
// =============================================================================

/// The disposition of one raw codex stream event.
#[derive(Debug)]
pub(crate) enum CodexMapped {
    /// No string `type`: skipped before any start emission.
    Skip,
    /// A pass-through Responses event.
    Event(ResponsesStreamEvent),
    /// A terminal event normalized to `response.completed`.
    Terminal(ResponsesStreamEvent),
    /// An `error`/`response.failed` event: a codex API error.
    Error(CodexStreamError),
}

/// Upstream `mapCodexEvents` (lines 725-762) as a per-event function.
pub(crate) fn map_codex_event(event: &Value, end_turn: &mut Option<bool>) -> CodexMapped {
    let Some(event_type) = event.get("type").and_then(Value::as_str) else {
        return CodexMapped::Skip;
    };

    if event_type == "error" {
        let (code, message) = extract_codex_event_error(event);
        let detail = message
            .clone()
            .or_else(|| code.clone())
            .unwrap_or_else(|| serde_json::to_string(event).unwrap_or_default());
        return CodexMapped::Error(CodexStreamError::api(
            code,
            format!("Codex error: {detail}"),
        ));
    }

    if event_type == "response.failed" {
        let response_error = event
            .get("response")
            .and_then(|response| response.get("error"));
        let code = response_error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let message = response_error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string);
        return CodexMapped::Error(CodexStreamError::api(
            code,
            message.unwrap_or_else(|| "Codex response failed".to_string()),
        ));
    }

    if matches!(
        event_type,
        "response.done" | "response.completed" | "response.incomplete"
    ) {
        let response = event.get("response");
        if let Some(value) = response.and_then(|response| response.get("end_turn")) {
            if let Some(end) = value.as_bool() {
                *end_turn = Some(end);
            }
        }
        // `normalizeCodexStatus`: unknown statuses disappear from the
        // response object; `response.done`/`incomplete` become
        // `response.completed` for the shared processor.
        let mut normalized = response.cloned().unwrap_or(Value::Null);
        if let Value::Object(map) = &mut normalized {
            let known = map
                .get("status")
                .and_then(Value::as_str)
                .map(|status| CODEX_RESPONSE_STATUSES.contains(&status))
                .unwrap_or(false);
            if !known {
                map.shift_remove("status");
            }
        }
        let mut mapped = event.clone();
        if let Value::Object(map) = &mut mapped {
            map.insert("type".into(), json!("response.completed"));
            map.insert("response".into(), normalized);
        }
        return CodexMapped::Terminal(ResponsesStreamEvent::Completed {
            response: ResponsesResponse::from_raw(mapped.get("response")),
        });
    }

    CodexMapped::Event(ResponsesStreamEvent::from_value(event.clone()))
}

/// Upstream `extractCodexEventError` (lines 712-723): top-level string
/// fields, else the nested `error` object's.
fn extract_codex_event_error(event: &Value) -> (Option<String>, Option<String>) {
    let nested = event.get("error").filter(|error| error.is_object());
    let read = |key: &str| {
        event
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                nested
                    .and_then(|nested| nested.get(key))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    };
    (read("code"), read("message"))
}

// =============================================================================
// Retry policy (upstream lines 123-198)
// =============================================================================

/// Upstream `isTerminalRateLimitError` (lines 123-127) — the same pattern
/// family as the assistant-call retry's non-retryable list.
fn is_terminal_rate_limit_error(error_text: &str) -> bool {
    crate::ai::retry::is_non_retryable_provider_limit_error(error_text)
}

const RETRYABLE_TEXT_PATTERNS: [&str; 5] = [
    "rate.?limit",
    "overloaded",
    "service.?unavailable",
    "upstream.?connect",
    "connection.?refused",
];

/// Upstream `isRetryableError` (lines 129-137).
fn is_retryable_error(status: u16, error_text: &str) -> bool {
    if status == 429 && is_terminal_rate_limit_error(error_text) {
        return false;
    }
    if matches!(status, 429 | 500 | 502 | 503 | 504) {
        return true;
    }
    RETRYABLE_TEXT_PATTERNS
        .iter()
        .any(|pattern| pattern_matches(pattern, error_text))
}

/// Upstream `getRetryAfterDelayMs` (lines 139-164). `now_ms` feeds the
/// HTTP-date branch. Micro-deviation: upstream's `Number("")`-style coercion
/// turns a whitespace-only `retry-after` into `0`; the port skips it.
fn get_retry_after_delay_ms(headers: &reqwest::header::HeaderMap, now_ms: u64) -> Option<u64> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    if let Some(value) = header("retry-after-ms") {
        if let Ok(millis) = value.parse::<f64>() {
            if millis.is_finite() {
                return Some((millis.max(0.0)) as u64);
            }
        }
    }
    let retry_after = header("retry-after")?;
    if retry_after.is_empty() {
        return None;
    }
    if let Ok(seconds) = retry_after.parse::<f64>() {
        if seconds.is_finite() {
            return Some((seconds.max(0.0) as u64).saturating_mul(1000));
        }
    }
    parse_http_date(retry_after).map(|epoch_ms| epoch_ms.saturating_sub(now_ms))
}

/// IMF-fixdate parsing for the `retry-after` HTTP-date branch (upstream
/// `Date.parse` also accepts the obsolete RFC 850 / asctime forms; Retry-After
/// senders use IMF-fixdate). Shared with the github-copilot OAuth flow's
/// rate-limit retry (same upstream `Retry-After` semantics).
pub(crate) fn parse_http_date(value: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, rest) = value.split_once(", ")?;
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() != 5 || parts[4] != "GMT" {
        return None;
    }
    let day: i64 = parts[0].parse().ok()?;
    let month = MONTHS.iter().position(|month| *month == parts[1])? as i64 + 1;
    let year: i64 = parts[2].parse().ok()?;
    let mut time = parts[3].split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next()?.parse().ok()?;
    if time.next().is_some() {
        return None;
    }
    // Days from the civil epoch (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(
        (days * 86_400 + hour * 3_600 + minute * 60 + second)
            .unsigned_abs()
            .saturating_mul(1000),
    )
}

/// Upstream `validateRetryDelayMs` (lines 168-176): a server-requested delay
/// above the cap fails the request with the exact upstream message.
fn validate_retry_delay_ms(
    delay_ms: u64,
    max_retry_delay_ms: Option<u64>,
) -> Result<u64, CodexStreamError> {
    let max = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
    if max > 0 && delay_ms > max {
        return Err(CodexStreamError::retry_delay_exceeded(format!(
            "Server requested {}s retry delay (max: {}s)",
            delay_ms.div_ceil(1000),
            max.div_ceil(1000),
        )));
    }
    Ok(delay_ms)
}

/// Upstream `BASE_DELAY_MS * 2 ** attempt` (lines 428, 455).
fn base_delay_ms(attempt: u32) -> u64 {
    BASE_DELAY_MS.saturating_mul(1u64 << attempt.min(62))
}

/// The parsed body of an HTTP error response (upstream `parseErrorResponse`,
/// lines 1565-1590).
struct ParsedErrorResponse {
    message: String,
    friendly_message: Option<String>,
}

/// Upstream `parseErrorResponse`: the ChatGPT usage-limit rendering and the
/// plain message passthrough.
fn parse_error_response(status: u16, raw: &str) -> ParsedErrorResponse {
    let mut message = if !raw.is_empty() {
        raw.to_string()
    } else {
        reqwest::StatusCode::from_u16(status)
            .ok()
            .and_then(|status| status.canonical_reason())
            .unwrap_or("Request failed")
            .to_string()
    };
    let mut friendly_message: Option<String> = None;
    if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
        if let Some(error) = parsed.get("error").filter(|error| error.is_object()) {
            let read = |key: &str| {
                error
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            };
            let code = read("code").or_else(|| read("type")).unwrap_or("");
            let usage_limit = [
                "usage_limit_reached",
                "usage_not_included",
                "rate_limit_exceeded",
            ]
            .iter()
            .any(|pattern| code.to_lowercase().contains(pattern))
                || status == 429;
            if usage_limit {
                let plan = read("plan_type")
                    .map(|plan| format!(" ({} plan)", plan.to_lowercase()))
                    .unwrap_or_default();
                let resets_at = error
                    .get("resets_at")
                    .and_then(Value::as_f64)
                    .filter(|resets_at| *resets_at != 0.0);
                let when = match resets_at {
                    Some(resets_at) => {
                        let minutes = ((resets_at * 1000.0 - now_ms() as f64) / 60_000.0)
                            .round()
                            .max(0.0);
                        format!(" Try again in ~{minutes} min.")
                    }
                    None => String::new(),
                };
                friendly_message = Some(
                    format!("You have hit your ChatGPT usage limit{plan}.{when}")
                        .trim()
                        .to_string(),
                );
            }
            if let Some(error_message) = read("message") {
                message = error_message.to_string();
            } else if let Some(friendly) = &friendly_message {
                message = friendly.clone();
            }
        }
    }
    ParsedErrorResponse {
        message,
        friendly_message,
    }
}

// =============================================================================
// URL resolution (upstream lines 641-654)
// =============================================================================

/// Upstream `resolveCodexUrl`: normalize the base URL onto the
/// `/codex/responses` endpoint.
fn resolve_codex_url(base_url: Option<&str>) -> String {
    let raw = match base_url {
        Some(base_url) if !base_url.trim().is_empty() => base_url,
        _ => DEFAULT_CODEX_BASE_URL,
    };
    let normalized = raw.trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        normalized.to_string()
    } else if normalized.ends_with("/codex") {
        format!("{normalized}/responses")
    } else {
        format!("{normalized}/codex/responses")
    }
}

/// Upstream `resolveCodexWebSocketUrl`: the resolved endpoint with the
/// protocol swapped to ws/wss.
fn resolve_codex_websocket_url(base_url: Option<&str>) -> String {
    let mut url: reqwest::Url = resolve_codex_url(base_url)
        .parse()
        .expect("resolve_codex_url yields a valid URL for valid bases");
    if url.scheme() == "https" {
        let _ = url.set_scheme("wss");
    } else if url.scheme() == "http" {
        let _ = url.set_scheme("ws");
    }
    url.to_string()
}

// =============================================================================
// Auth and headers (upstream lines 1596-1666)
// =============================================================================

/// Upstream `extractAccountId` (lines 1596-1607): the JWT payload's
/// `chatgpt_account_id` claim; every failure collapses to the upstream
/// message.
fn extract_account_id(token: &str) -> Result<String, CodexStreamError> {
    let failure = || CodexStreamError::plain("Failed to extract accountId from token");
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(failure());
    }
    let payload = decode_standard_base64(parts[1]).ok_or_else(failure)?;
    let parsed: Value = serde_json::from_slice(&payload).map_err(|_| failure())?;
    parsed
        .get(JWT_CLAIM_PATH)
        .and_then(|claim| claim.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(failure)
}

/// `atob`-equivalent decoding of the standard base64 alphabet (JWT payloads).
/// `pub(crate)`: the oauth flow port (`auth::oauth::openai_codex`) decodes
/// id/access-token payload segments with the same `atob` semantics.
pub(crate) fn decode_standard_base64(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for character in input.chars().filter(|c| !c.is_ascii_whitespace()) {
        if character == '=' {
            continue;
        }
        let value = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            'a'..='z' => character as u32 - 'a' as u32 + 26,
            '0'..='9' => character as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
        }
    }
    (bits < 6).then_some(output)
}

/// Upstream `buildBaseCodexHeaders` (lines 1609-1628): model headers, then
/// the caller's (null suppresses), then the four identity headers, which
/// override anything the caller set (unlike the sibling ports' SDK flow).
/// `set`/`remove` are case-insensitive like the JS `Headers` object.
fn build_base_codex_headers(
    model: &Model,
    options: &SimpleStreamOptions,
    account_id: &str,
    token: &str,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = Vec::new();
    for (name, value) in model.headers.iter().flatten() {
        match value {
            Some(value) => set_header(&mut headers, name, value),
            None => remove_header(&mut headers, name),
        }
    }
    if let Some(option_headers) = &options.stream.headers {
        for (name, value) in option_headers {
            match value {
                Some(value) => set_header(&mut headers, name, value),
                None => remove_header(&mut headers, name),
            }
        }
    }
    set_header(&mut headers, "Authorization", &format!("Bearer {token}"));
    set_header(&mut headers, "chatgpt-account-id", account_id);
    set_header(&mut headers, "originator", "pi");
    set_header(&mut headers, "User-Agent", &pi_user_agent());
    headers
}

/// Upstream `buildSSEHeaders` (lines 1630-1648).
fn build_sse_headers(
    model: &Model,
    options: &SimpleStreamOptions,
    account_id: &str,
    token: &str,
    session_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = build_base_codex_headers(model, options, account_id, token);
    set_header(&mut headers, "OpenAI-Beta", "responses=experimental");
    set_header(&mut headers, "accept", "text/event-stream");
    set_header(&mut headers, "content-type", "application/json");
    if let Some(session_id) = session_id {
        set_header(&mut headers, "session-id", session_id);
        set_header(&mut headers, "x-client-request-id", session_id);
    }
    headers
}

/// Upstream `buildWebSocketHeaders` (lines 1650-1666) plus the
/// `headersToRecord` lowercasing `connectWebSocket` applies (see the
/// [`websocket`] module docs for why the `OpenAI-Beta` delete there is a
/// no-op upstream).
fn build_websocket_headers(
    model: &Model,
    options: &SimpleStreamOptions,
    account_id: &str,
    token: &str,
    request_id: &str,
) -> Vec<(String, String)> {
    let mut headers = build_base_codex_headers(model, options, account_id, token);
    remove_header(&mut headers, "accept");
    remove_header(&mut headers, "content-type");
    remove_header(&mut headers, "openai-beta");
    set_header(
        &mut headers,
        "OpenAI-Beta",
        OPENAI_BETA_RESPONSES_WEBSOCKETS,
    );
    set_header(&mut headers, "x-client-request-id", request_id);
    set_header(&mut headers, "session-id", request_id);
    headers
        .into_iter()
        .map(|(name, value)| (name.to_lowercase(), value))
        .collect()
}

/// Case-insensitive `Headers.set` over the ordered header list.
fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    match headers
        .iter_mut()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
    {
        Some(entry) => entry.1 = value.to_string(),
        None => headers.push((name.to_string(), value.to_string())),
    }
}

/// Case-insensitive `Headers.delete`.
fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
}

// =============================================================================
// Request body (upstream buildRequestBody, lines 527-600)
// =============================================================================

/// Upstream `buildRequestBody` over an already-normalized transcript.
#[allow(clippy::too_many_arguments)]
fn build_request_body(
    model: &Model,
    context: &TranscriptContext,
    options: &SimpleStreamOptions,
    codex: &CodexRequestOptions,
    cache_session_id: Option<&str>,
    grammar_tool_input_properties: &std::collections::HashMap<String, String>,
) -> Result<Value, String> {
    let compat = model.openai_responses_compat().unwrap_or_default();
    let supports_strict_mode = compat.supports_strict_mode.unwrap_or(true);
    let supports_openai_grammar_tools = compat.supports_openai_grammar_tools.unwrap_or(false);
    let supports_additional_tools = compat.supports_additional_tools.unwrap_or(false);
    let supports_tool_search = compat.supports_tool_search.unwrap_or(false);
    let supports_mid_convo = compat.supports_mid_convo_system_messages.unwrap_or(false);

    let transcript_tools = resolve_transcript_tools(
        context.messages(),
        supports_additional_tools || supports_tool_search,
    );
    let tool_options = ConvertResponsesToolsOptions {
        // Upstream line 548: `strict: null` drops the SDK default on the
        // codex endpoint; grammar tools still force `strict: true` in the
        // shared converter.
        strict: Some(None),
        supports_strict_mode,
        supports_openai_grammar_tools,
        tool_search_result: false,
    };
    let allowed: HashSet<String> = CODEX_TOOL_CALL_PROVIDERS
        .iter()
        .map(|provider| (*provider).to_string())
        .collect();
    let messages = convert_responses_messages(
        model,
        context,
        &allowed,
        &ConvertResponsesMessagesOptions {
            include_system_prompt: Some(false),
            grammar_tool_input_properties: grammar_tool_input_properties.clone(),
            supports_mid_convo_system_messages: supports_mid_convo,
            supports_additional_tools,
            supports_tool_search,
            tool_options: Some(tool_options.clone()),
        },
    )?;

    // Upstream lines 551-552: the leading system prompt rides `instructions`.
    let instructions = get_initial_system_message(context.messages())
        .map(get_system_message_text)
        .unwrap_or_default();

    let mut body = Map::new();
    body.insert("model".into(), json!(model.id));
    body.insert("store".into(), json!(false));
    body.insert("stream".into(), json!(true));
    body.insert(
        "instructions".into(),
        json!(if instructions.is_empty() {
            DEFAULT_INSTRUCTIONS
        } else {
            instructions.as_str()
        }),
    );
    body.insert("input".into(), Value::Array(messages));
    body.insert(
        "text".into(),
        json!({
            "verbosity": codex
                .text_verbosity
                .clone()
                .filter(|verbosity| !verbosity.is_empty())
                .unwrap_or_else(|| "low".to_string())
        }),
    );
    body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    if let Some(cache_session_id) = cache_session_id {
        body.insert("prompt_cache_key".into(), json!(cache_session_id));
    }
    body.insert(
        "tool_choice".into(),
        json!(codex
            .tool_choice
            .clone()
            .unwrap_or_else(|| "auto".to_string())),
    );
    body.insert("parallel_tool_calls".into(), json!(true));

    if let Some(temperature) = options.stream.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(service_tier) = &codex.service_tier {
        body.insert("service_tier".into(), json!(service_tier));
    }
    if !transcript_tools.request_tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(convert_responses_tools(
                &transcript_tools.request_tools,
                &tool_options,
            )?),
        );
    }

    // Upstream lines 582-597: the reasoning effort mapping. `reasoningSummary`
    // always renders "auto" (no port option surface).
    if let Some(requested) = &codex.reasoning_effort {
        let effort: Option<String> = if requested == "none" {
            match map_level(model, "off") {
                MappedLevel::Absent => Some("none".to_string()),
                MappedLevel::Null => None,
                MappedLevel::Value(value) => Some(value),
            }
        } else {
            Some(match map_level(model, requested) {
                // JS `thinkingLevelMap[effort] ?? effort`: null falls back to
                // the requested level string.
                MappedLevel::Value(value) => value,
                MappedLevel::Null | MappedLevel::Absent => requested.clone(),
            })
        };
        if let Some(effort) = effort {
            body.insert(
                "reasoning".into(),
                json!({"effort": effort, "summary": "auto"}),
            );
        }
    } else if model.reasoning && map_level(model, "off") != MappedLevel::Null {
        let effort = match map_level(model, "off") {
            MappedLevel::Value(value) => value,
            MappedLevel::Null | MappedLevel::Absent => "none".to_string(),
        };
        body.insert("reasoning".into(), json!({"effort": effort}));
    }

    Ok(Value::Object(body))
}

// =============================================================================
// Service tier pricing (upstream lines 602-639)
// =============================================================================

/// Upstream `getServiceTierCostMultiplier` (lines 602-614).
fn get_service_tier_cost_multiplier(model_id: &str, service_tier: Option<&str>) -> f64 {
    match service_tier {
        Some("flex") => 0.5,
        Some("priority") => {
            if model_id == "gpt-5.5" {
                2.5
            } else {
                2.0
            }
        }
        _ => 1.0,
    }
}

/// Upstream `applyServiceTierPricing` (lines 616-629).
fn apply_service_tier_pricing(usage: &mut Usage, service_tier: Option<&str>, model_id: &str) {
    let multiplier = get_service_tier_cost_multiplier(model_id, service_tier);
    if multiplier == 1.0 {
        return;
    }
    usage.cost.input *= multiplier;
    usage.cost.output *= multiplier;
    usage.cost.cache_read *= multiplier;
    usage.cost.cache_write *= multiplier;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

/// Upstream `resolveCodexServiceTier` (lines 631-639): a client-requested
/// flex/priority tier wins over a bare "default" echo.
fn resolve_codex_service_tier(response: Option<&str>, request: Option<&str>) -> Option<String> {
    if response == Some("default") && matches!(request, Some("flex") | Some("priority")) {
        return request.map(str::to_string);
    }
    response
        .map(str::to_string)
        .or_else(|| request.map(str::to_string))
}

// =============================================================================
// Request compression (upstream lines 208-231)
// =============================================================================

/// Upstream `compressRequestBodyZstd` at level 3. The "no zlib runtime"
/// fallback only maps to an encode failure here; the compressed body is
/// always preferred.
fn compress_request_body_zstd(body_json: &str) -> Option<Vec<u8>> {
    zstd::stream::encode_all(body_json.as_bytes(), REQUEST_COMPRESSION_ZSTD_LEVEL).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::message::{
        AssistantBlock, Message, StringOrBlocks, TextOrImageBlock, UserMessage,
    };
    use crate::ai::types::primitives::ModelCost;
    use crate::ai::types::tool::{ConstrainedSampling, GrammarSampling, Strict, Tool};
    use crate::ai::types::{Model, ModelInput};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use websocket::WsEventKind;

    const API: &str = "openai-codex-responses";
    const TS: i64 = 1758240000000;

    // ---- fixtures ----

    /// A JWT-shaped bearer token carrying the ChatGPT account claim.
    fn mock_token(account_id: &str) -> String {
        let payload = base64_encode(
            &serde_json::to_vec(&json!({ JWT_CLAIM_PATH: { "chatgpt_account_id": account_id } }))
                .unwrap(),
        );
        format!("aaa.{payload}.bbb")
    }

    fn base64_encode(input: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let mut bytes = [0u8; 3];
            bytes[..chunk.len()].copy_from_slice(chunk);
            let packed = ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | bytes[2] as u32;
            out.push(ALPHABET[(packed >> 18) as usize & 63] as char);
            out.push(ALPHABET[(packed >> 12) as usize & 63] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[(packed >> 6) as usize & 63] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[packed as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    fn model() -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "gpt-5.1-codex".to_string(),
            name: "GPT-5.1 Codex".to_string(),
            api: API.to_string(),
            provider: "openai-codex".to_string(),
            base_url: "https://chatgpt.com/backend-api".to_string(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: ModelCost::default(),
            context_window: 400000,
            max_tokens: 128000,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    fn model_with_id(id: &str) -> Model {
        let mut model = model();
        model.id = id.to_string();
        model
    }

    fn model_with_map(map: serde_json::Value) -> Model {
        let mut model = model();
        model.thinking_level_map = Some(serde_json::from_value(map).unwrap());
        model
    }

    fn model_on(server: &wiremock::MockServer) -> Model {
        let mut model = model();
        model.base_url = server.uri();
        model
    }

    fn cfg() -> ProviderConfig {
        ProviderConfig {
            base_url: "https://chatgpt.com/backend-api".to_string(),
            api_key: String::new(),
            max_tokens: 8192,
        }
    }

    fn ctx_with(system_prompt: Option<&str>, messages: Vec<Message>) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: system_prompt.map(str::to_string),
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

    /// The oracle's `buildSSEPayload`.
    fn sse_payload(status: &str, include_done: bool, end_turn: Option<bool>) -> String {
        let terminal_type = if status == "incomplete" {
            "response.incomplete"
        } else {
            "response.completed"
        };
        let mut response = json!({
            "status": status,
            "usage": {
                "input_tokens": 5,
                "output_tokens": 3,
                "total_tokens": 8,
                "input_tokens_details": {"cached_tokens": 0},
            },
        });
        if let Some(end_turn) = end_turn {
            response["end_turn"] = json!(end_turn);
        }
        if status == "incomplete" {
            response["incomplete_details"] = json!({"reason": "max_output_tokens"});
        }
        let mut events = vec![
            json!({
                "type": "response.output_item.added",
                "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []},
            }),
            json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": ""}}),
            json!({"type": "response.output_text.delta", "delta": "Hello"}),
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": "Hello"}],
                },
            }),
            json!({"type": terminal_type, "response": response}),
        ];
        if include_done {
            events.push(json!("[DONE]"));
        }
        events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect()
    }

    fn sse(events: &[serde_json::Value]) -> String {
        events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect()
    }

    fn completed_sse() -> String {
        sse_payload("completed", false, None)
    }

    fn sse_response(body: String) -> wiremock::ResponseTemplate {
        wiremock::ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body)
    }

    async fn mount_codex(server: &wiremock::MockServer, body: String) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/codex/responses"))
            .respond_with(sse_response(body))
            .mount(server)
            .await;
    }

    /// First request gets `error`, later requests succeed.
    struct Flaky {
        attempts: AtomicU32,
        error: wiremock::ResponseTemplate,
        success: wiremock::ResponseTemplate,
    }

    impl wiremock::Respond for Flaky {
        fn respond(&self, _request: &wiremock::Request) -> wiremock::ResponseTemplate {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                self.error.clone()
            } else {
                self.success.clone()
            }
        }
    }

    fn sse_options(overrides: impl FnOnce(&mut SimpleStreamOptions)) -> SimpleStreamOptions {
        let mut options = SimpleStreamOptions {
            stream: StreamOptions {
                api_key: Some(mock_token("acc_test")),
                transport: Some(Transport::Sse),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        overrides(&mut options);
        options
    }

    async fn collect(
        _server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = OpenAiCodexResponses;
        let mut rx = api.stream_simple(&cfg(), model, ctx, options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    async fn collect_codex(
        _server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
        codex: CodexRequestOptions,
    ) -> Vec<AssistantMessageEvent> {
        let mut rx =
            stream_with_codex_options(cfg(), model.clone(), ctx.clone(), options.clone(), codex);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    /// Runs one stream; returns (last request body as JSON, its headers,
    /// events).
    async fn capture(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> (
        Value,
        reqwest::header::HeaderMap,
        Vec<AssistantMessageEvent>,
    ) {
        let events = collect(server, model, ctx, options).await;
        let requests = server.received_requests().await.unwrap();
        let request = requests.last().expect("at least one request");
        let body = decode_request_body(request);
        (body, request.headers.clone(), events)
    }

    // ---- abort surface (openai-codex-stream.test.ts abort oracles) ----

    /// Oracle "aborts SSE body reads after response headers arrive"
    /// (openai-codex-stream.test.ts:424-527): a paced raw-TCP SSE stream
    /// delivers one text delta; cancelling then settles the stream aborted
    /// and no further event reaches the consumer. The raw listener stands in
    /// for the server so the second batch stays buffered until after the
    /// cancellation (wiremock cannot pace a body).
    #[tokio::test]
    async fn aborts_sse_body_reads_after_response_headers_arrive() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let batch_two = tokio::sync::oneshot::channel::<()>();
        let (batch_two_tx, batch_two_rx) = batch_two;
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let mut body = String::new();
            for event in [
                json!({
                    "type": "response.output_item.added",
                    "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []},
                }),
                json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": ""}}),
                json!({"type": "response.output_text.delta", "delta": "one"}),
            ] {
                body.push_str(&format!("data: {event}\n\n"));
            }
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                .await;
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.flush().await;
            // Wait for the test to release the second batch, then stream the
            // remaining events the cancelled client must never see.
            let _ = batch_two_rx.await;
            let mut rest = String::new();
            for event in [
                json!({"type": "response.output_text.delta", "delta": "two"}),
                json!({
                    "type": "response.completed",
                    "response": {
                        "status": "completed",
                        "usage": {"input_tokens": 5, "output_tokens": 3, "total_tokens": 8, "input_tokens_details": {"cached_tokens": 0}},
                    },
                }),
            ] {
                rest.push_str(&format!("data: {event}\n\n"));
            }
            let _ = socket.write_all(rest.as_bytes()).await;
            let _ = socket.shutdown().await;
        });

        let mut model = model();
        model.base_url = format!("http://{addr}");
        let ctx = ctx_with(None, vec![user_msg("Say hello")]);
        let token = CancellationToken::new();
        let options = sse_options(|options| {
            options.stream.signal = Some(token.clone());
        });

        let api = OpenAiCodexResponses;
        let mut rx = api.stream_simple(&cfg(), &model, &ctx, &options);
        let mut deltas: Vec<String> = Vec::new();
        let mut terminal: Option<AssistantMessageEvent> = None;
        while terminal.is_none() {
            match rx.recv().await {
                Some(AssistantMessageEvent::TextDelta { delta, .. }) => {
                    deltas.push(delta);
                    if deltas.last().map(String::as_str) == Some("one") {
                        token.cancel();
                    }
                }
                Some(event @ AssistantMessageEvent::Done { .. })
                | Some(event @ AssistantMessageEvent::Error { .. }) => terminal = Some(event),
                Some(_) => {}
                None => panic!("stream ended without a terminal event"),
            }
        }
        let terminal = terminal.expect("terminal event");
        // Release the second batch, then give the (already dropped) client
        // connection a moment to prove no further deltas arrive.
        let _ = batch_two_tx.send(());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(deltas, vec!["one".to_string()]);
        match terminal {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
            }
            other => panic!("expected terminal error event, got {other:?}"),
        }
    }

    /// A pre-cancelled signal fails the SSE attempt before the fetch
    /// (upstream line 397) and settles the catch block's aborted branch.
    #[tokio::test]
    async fn pre_aborted_codex_stream_settles_aborted_without_a_request() {
        let server = wiremock::MockServer::start().await;
        let model = model_on(&server);
        let ctx = ctx_with(None, vec![user_msg("Say hello")]);
        let token = CancellationToken::new();
        token.cancel();
        let options = sse_options(|options| {
            options.stream.signal = Some(token);
        });

        let api = OpenAiCodexResponses;
        let mut rx = api.stream_simple(&cfg(), &model, &ctx, &options);
        let first = rx.recv().await.expect("terminal event");
        match first {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
            }
            other => panic!("expected terminal error event, got {other:?}"),
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// Decodes the request body (zstd when `content-encoding` says so).
    fn decode_request_body(request: &wiremock::Request) -> Value {
        if request
            .headers
            .get("content-encoding")
            .map(|value| value == "zstd")
            .unwrap_or(false)
        {
            let decoded = zstd::stream::decode_all(&request.body[..]).unwrap();
            serde_json::from_slice(&decoded).unwrap()
        } else {
            serde_json::from_slice(&request.body).unwrap()
        }
    }

    fn event_types(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(AssistantMessageEvent::event_type)
            .collect()
    }

    fn error_of(events: &[AssistantMessageEvent]) -> AssistantMessage {
        match events.last() {
            Some(AssistantMessageEvent::Error { error, .. }) => error.clone(),
            other => panic!("expected terminal error, got {other:?}"),
        }
    }

    fn header_value<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn done_message(events: &[AssistantMessageEvent]) -> AssistantMessage {
        match events.last() {
            Some(AssistantMessageEvent::Done { message, .. }) => message.clone(),
            other => panic!("expected done, got {other:?}"),
        }
    }

    // ---- 1. SSE transport + headers (oracle "streams SSE responses...") ----

    #[tokio::test]
    async fn streams_sse_responses_with_codex_headers() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|_| {});
        let events = collect(&server, &model, &ctx, &options).await;

        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "text_end", "done"],
            "{events:?}"
        );
        let message = done_message(&events);
        match message.content.first() {
            Some(AssistantBlock::Text(text)) => assert_eq!(text.text, "Hello"),
            other => panic!("expected text block, got {other:?}"),
        }
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.usage.input, 5);
        assert_eq!(message.usage.output, 3);
        assert_eq!(message.usage.total_tokens, 8);

        let requests = server.received_requests().await.unwrap();
        let headers = &requests[0].headers;
        let token = mock_token("acc_test");
        assert_eq!(
            header_value(headers, "authorization"),
            Some(format!("Bearer {token}").as_str())
        );
        assert_eq!(
            header_value(headers, "chatgpt-account-id"),
            Some("acc_test")
        );
        assert_eq!(
            header_value(headers, "openai-beta"),
            Some("responses=experimental")
        );
        assert_eq!(header_value(headers, "originator"), Some("pi"));
        assert_eq!(
            header_value(headers, "user-agent"),
            Some(pi_user_agent().as_str())
        );
        assert_eq!(header_value(headers, "accept"), Some("text/event-stream"));
        assert_eq!(
            header_value(headers, "content-type"),
            Some("application/json")
        );
        assert!(headers.get("x-api-key").is_none());

        let body = decode_request_body(&requests[0]);
        assert_eq!(body["model"], json!("gpt-5.1-codex"));
        assert_eq!(body["store"], json!(false));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["instructions"], json!("You are a helpful assistant."));
        assert_eq!(
            body["input"],
            json!([{"role": "user", "content": [{"type": "input_text", "text": "Say hello"}]}])
        );
        assert_eq!(body["text"], json!({"verbosity": "low"}));
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert!(body.get("prompt_cache_key").is_none());
    }

    /// The oracle's issue-9047 regression: a terminal SSE event without a
    /// trailing blank line still lands.
    #[tokio::test]
    async fn processes_terminal_sse_event_without_trailing_blank_line() {
        let server = wiremock::MockServer::start().await;
        let trimmed = completed_sse().trim_end().to_string();
        mount_codex(&server, trimmed).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let events = collect(&server, &model, &ctx, &sse_options(|_| {})).await;
        let message = done_message(&events);
        assert_eq!(message.stop_reason, StopReason::Stop);
        match message.content.first() {
            Some(AssistantBlock::Text(text)) => assert_eq!(text.text, "Hello"),
            other => panic!("expected text block, got {other:?}"),
        }
    }

    /// After the mapped terminal event, processing stops even if the body
    /// would keep going — a trailing `error` event must be invisible, and
    /// `end_turn` lands on the message (oracle "completes after
    /// response.completed even when the SSE body stays open").
    #[tokio::test]
    async fn completes_after_terminal_ignoring_later_events() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}data: {}\n\n",
            sse_payload("completed", true, Some(false)),
            json!({"type": "error", "message": "late failure"})
        );
        mount_codex(&server, body).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let events = collect(&server, &model, &ctx, &sse_options(|_| {})).await;
        assert_eq!(event_types(&events).last(), Some(&"done"));
        let message = done_message(&events);
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.end_turn, Some(false));
    }

    #[tokio::test]
    async fn maps_response_incomplete_to_length() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, sse_payload("incomplete", false, None)).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let events = collect(&server, &model, &ctx, &sse_options(|_| {})).await;
        assert_eq!(event_types(&events).last(), Some(&"done"));
        let message = done_message(&events);
        assert_eq!(message.stop_reason, StopReason::Length);
        assert_eq!(
            message.raw_stop_reason.as_deref(),
            Some("incomplete.max_output_tokens")
        );
    }

    // ---- 2. timeouts ----

    #[tokio::test]
    async fn aborts_sse_after_the_configured_headers_timeout() {
        let server = wiremock::MockServer::start().await;
        // The 5s server delay vs the 10ms client deadline makes the race
        // deterministic: the M2c-era 500ms delay could lose on a loaded
        // current-thread runtime if the deadline was polled >490ms late, and
        // the response then arrived first. The connection still aborts at
        // 10ms, so the test stays fast. (A paused tokio clock is not usable
        // here — reqwest and the mock server run on real I/O.)
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/codex/responses"))
            .respond_with(
                sse_response(completed_sse()).set_delay(std::time::Duration::from_millis(5000)),
            )
            .mount(&server)
            .await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| options.stream.timeout_ms = Some(10));
        let events = collect(&server, &model, &ctx, &options).await;
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let error = error_of(&events);
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Codex SSE response headers timed out after 10ms")
        );
    }

    // ---- 3. session affinity ----

    #[tokio::test]
    async fn sets_session_headers_and_prompt_cache_key_from_session_id() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.session_id = Some("test-session-123".to_string());
        });
        let (body, headers, events) = capture(&server, &model, &ctx, &options).await;
        assert_eq!(
            header_value(&headers, "session-id"),
            Some("test-session-123")
        );
        assert!(headers.get("session_id").is_none());
        assert_eq!(
            header_value(&headers, "x-client-request-id"),
            Some("test-session-123")
        );
        assert_eq!(body["prompt_cache_key"], json!("test-session-123"));
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    #[tokio::test]
    async fn omits_cache_affinity_when_cache_retention_is_none() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.session_id = Some("one-off-summary".to_string());
            options.stream.cache_retention = Some(CacheRetention::None);
        });
        let (body, headers, events) = capture(&server, &model, &ctx, &options).await;
        assert!(headers.get("session-id").is_none());
        assert!(headers.get("x-client-request-id").is_none());
        assert!(body.get("prompt_cache_key").is_none());
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    #[tokio::test]
    async fn clamps_prompt_cache_key_to_64_characters() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.session_id = Some("x".repeat(67));
        });
        let (body, headers, _) = capture(&server, &model, &ctx, &options).await;
        assert_eq!(body["prompt_cache_key"], json!("x".repeat(64)));
        assert_eq!(
            header_value(&headers, "session-id"),
            Some("x".repeat(64).as_str())
        );
        assert_eq!(
            header_value(&headers, "x-client-request-id"),
            Some("x".repeat(64).as_str())
        );
    }

    #[tokio::test]
    async fn omits_session_headers_without_session_id() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let (_, headers, events) = capture(&server, &model, &ctx, &sse_options(|_| {})).await;
        assert!(headers.get("session-id").is_none());
        assert!(headers.get("session_id").is_none());
        assert!(headers.get("x-client-request-id").is_none());
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    // ---- 4. reasoning + tools + tiers ----

    #[tokio::test]
    async fn preserves_xhigh_effort_from_simple_options() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let mut model = model_with_map(json!({"xhigh": "xhigh"}));
        model.base_url = server.uri();
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options =
            sse_options(|options| options.reasoning = Some(crate::ai::types::ThinkingLevel::Xhigh));
        let (body, _, events) = capture(&server, &model, &ctx, &options).await;
        assert_eq!(
            body["reasoning"],
            json!({"effort": "xhigh", "summary": "auto"})
        );
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    /// Oracle "clamps minimal reasoning effort to low" — upstream passes
    /// `reasoningEffort` on direct `stream`; the port surfaces the same
    /// builder through the test seam.
    #[tokio::test]
    async fn maps_minimal_effort_through_thinking_level_map() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let mut model = model_with_map(json!({"minimal": "low"}));
        model.base_url = server.uri();
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|_| {});
        let mut rx = stream_with_codex_options(
            cfg(),
            model,
            ctx,
            options,
            CodexRequestOptions {
                reasoning_effort: Some("minimal".to_string()),
                ..Default::default()
            },
        );
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let requests = server.received_requests().await.unwrap();
        let body = decode_request_body(&requests[0]);
        assert_eq!(
            body["reasoning"],
            json!({"effort": "low", "summary": "auto"})
        );
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    #[tokio::test]
    async fn default_off_reasoning_sends_none_effort_for_reasoning_models() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let (body, _, _) = capture(&server, &model, &ctx, &sse_options(|_| {})).await;
        // streamSimple without reasoning: no explicit effort -> the else
        // branch -> {"effort": "none"} (map.off ?? "none").
        assert_eq!(body["reasoning"], json!({"effort": "none"}));
    }

    #[tokio::test]
    async fn off_mapped_null_omits_reasoning_for_non_effort_requests() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let mut model = model_with_map(json!({"off": null}));
        model.base_url = server.uri();
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let (body, _, _) = capture(&server, &model, &ctx, &sse_options(|_| {})).await;
        assert!(body.get("reasoning").is_none(), "{body}");
    }

    #[tokio::test]
    async fn off_mapped_value_is_used_for_default_off() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let mut model = model_with_map(json!({"off": "minimal"}));
        model.base_url = server.uri();
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let (body, _, _) = capture(&server, &model, &ctx, &sse_options(|_| {})).await;
        assert_eq!(body["reasoning"], json!({"effort": "minimal"}));
    }

    /// Oracle "forwards required tool choice" — the neutral options only
    /// carry auto/none, so the required case is pinned on the pure builder.
    #[test]
    fn build_request_body_forwards_required_tool_choice() {
        let model = model();
        let ctx = ctx_with(None, vec![user_msg("Do not call ping.")]);
        let codex = CodexRequestOptions {
            tool_choice: Some("required".to_string()),
            ..Default::default()
        };
        let body = build_request_body(
            &model,
            &ctx,
            &SimpleStreamOptions::default(),
            &codex,
            None,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(body["tool_choice"], json!("required"));
    }

    #[tokio::test]
    async fn stream_simple_forwards_tool_choice_none_with_tools() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let mut model = model();
        model.base_url = server.uri();
        let tool = Tool {
            name: "ping".into(),
            description: "Ping".into(),
            parameters: json!({"type": "object", "properties": {"value": {"type": "string"}}}),
            constrained_sampling: None,
        };
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("Do not call ping. Respond with text instead.")],
            tools: Some(vec![tool]),
        });
        let options = sse_options(|options| options.tool_choice = Some(ToolChoice::None));
        let (body, _, events) = capture(&server, &model, &ctx, &options).await;
        assert_eq!(body["tool_choice"], json!("none"));
        assert_eq!(body["tools"][0]["name"], json!("ping"));
        // Codex strict default: explicit null (line 548).
        assert_eq!(body["tools"][0]["strict"], json!(null));
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    /// Oracle "sets Codex strict mode explicitly and honors constrained
    /// sampling": optional tools keep `strict: null`, grammar-preferred tools
    /// ship `strict: true`.
    #[tokio::test]
    async fn sets_strict_null_and_honors_constrained_sampling() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let mut model = model();
        model.base_url = server.uri();
        let optional = Tool {
            name: "optional".into(),
            description: "Optional constrained sampling".into(),
            parameters: json!({"type": "object", "properties": {"value": {"type": "string"}}}),
            constrained_sampling: None,
        };
        let strict = Tool {
            name: "strict".into(),
            description: "Strict constrained sampling".into(),
            parameters: json!({"type": "object", "properties": {"value": {"type": "string"}}}),
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(
                crate::ai::types::tool::JsonSchemaSampling {
                    strict: Strict::Prefer,
                },
            )),
        };
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("Use a tool")],
            tools: Some(vec![optional, strict]),
        });
        let (body, _, events) = capture(&server, &model, &ctx, &sse_options(|_| {})).await;
        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["name"], json!("optional"));
        assert_eq!(body["tools"][0]["strict"], json!(null));
        assert_eq!(body["tools"][1]["name"], json!("strict"));
        assert_eq!(body["tools"][1]["strict"], json!(true));
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    /// Oracle "uses the client-sent service tier when Codex echoes default":
    /// four model/tier combinations with their pricing multipliers.
    #[tokio::test]
    async fn service_tier_pricing_multipliers() {
        #[derive(Debug)]
        struct Case {
            model_id: &'static str,
            tier: &'static str,
            multiplier: f64,
        }
        let cases = [
            Case {
                model_id: "gpt-5.1-codex",
                tier: "flex",
                multiplier: 0.5,
            },
            Case {
                model_id: "gpt-5.1-codex",
                tier: "priority",
                multiplier: 2.0,
            },
            Case {
                model_id: "gpt-5.5",
                tier: "flex",
                multiplier: 0.5,
            },
            Case {
                model_id: "gpt-5.5",
                tier: "priority",
                multiplier: 2.5,
            },
        ];
        let server = wiremock::MockServer::start().await;
        let events_body = sse(&[json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "service_tier": "default",
                "usage": {
                    "input_tokens": 1000000,
                    "output_tokens": 1000000,
                    "total_tokens": 2000000,
                    "input_tokens_details": {"cached_tokens": 0},
                },
            },
        })]);
        mount_codex(&server, events_body).await;

        for case in cases {
            let mut model = model_with_id(case.model_id);
            model.base_url = server.uri();
            model.cost = ModelCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: None,
            };
            let ctx = ctx_with(
                Some("You are a helpful assistant."),
                vec![user_msg("Say hello")],
            );
            let options = sse_options(|_| {});
            let events = collect_codex(
                &server,
                &model,
                &ctx,
                &options,
                CodexRequestOptions {
                    service_tier: Some(case.tier.to_string()),
                    ..Default::default()
                },
            )
            .await;
            let message = done_message(&events);
            assert!(
                (message.usage.cost.input - 1.0 * case.multiplier).abs() < 1e-9,
                "{case:?}"
            );
            assert!(
                (message.usage.cost.output - 2.0 * case.multiplier).abs() < 1e-9,
                "{case:?}"
            );
            assert!(
                (message.usage.cost.total - 3.0 * case.multiplier).abs() < 1e-9,
                "{case:?}"
            );
        }
        // One request per case plus the mount-time none.
        assert_eq!(server.received_requests().await.unwrap().len(), 4);
    }

    // ---- 5. setup errors ----

    #[tokio::test]
    async fn missing_api_key_is_a_lone_error_event() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| options.stream.api_key = None);
        let events = collect(&server, &model, &ctx, &options).await;
        assert_eq!(events.len(), 1, "{events:?}");
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("No API key for provider: openai-codex")
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn invalid_token_fails_account_id_extraction() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| options.stream.api_key = Some("not-a-jwt".to_string()));
        let events = collect(&server, &model, &ctx, &options).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Failed to extract accountId from token")
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    // ---- 6. SSE retry policy ----

    #[tokio::test]
    async fn retries_429_with_retry_after_ms_then_succeeds() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/codex/responses"))
            .respond_with(Flaky {
                attempts: AtomicU32::new(0),
                error: wiremock::ResponseTemplate::new(429)
                    .insert_header("retry-after-ms", "15")
                    .set_body_string(
                        r#"{"error": {"code": "rate_limit_exceeded", "message": "rate limited"}}"#,
                    ),
                success: sse_response(completed_sse()),
            })
            .mount(&server)
            .await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| options.stream.max_retries = Some(1));
        let events = collect(&server, &model, &ctx, &options).await;
        assert_eq!(event_types(&events).last(), Some(&"done"));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    /// Oracle "fails immediately when a retry delay exceeds the limit" (429
    /// and 503), with the exact upstream message.
    #[tokio::test]
    async fn retry_delay_above_limit_fails_immediately() {
        for status in [429u16, 503] {
            let server = wiremock::MockServer::start().await;
            wiremock::Mock::given(wiremock::matchers::method("POST"))
                .and(wiremock::matchers::path("/codex/responses"))
                .respond_with(
                    wiremock::ResponseTemplate::new(status)
                        .insert_header("retry-after", "2")
                        .set_body_string(r#"{"error": {"code": "temporarily_unavailable", "message": "retry later"}}"#),
                )
                .mount(&server)
                .await;
            let model = model_on(&server);
            let ctx = ctx_with(
                Some("You are a helpful assistant."),
                vec![user_msg("Say hello")],
            );
            let options = sse_options(|options| {
                options.stream.max_retries = Some(3);
                options.stream.max_retry_delay_ms = Some(1000);
            });
            let events = collect(&server, &model, &ctx, &options).await;
            let error = error_of(&events);
            assert_eq!(
                error.error_message.as_deref(),
                Some("Server requested 2s retry delay (max: 1s)"),
                "status {status}"
            );
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                1,
                "status {status}"
            );
        }
    }

    /// Terminal quota errors fail fast regardless of remaining retries.
    #[tokio::test]
    async fn terminal_rate_limit_error_is_not_retried() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/codex/responses"))
            .respond_with(
                wiremock::ResponseTemplate::new(429).set_body_string(
                    r#"{"error": {"code": "usage_limit_reached", "plan_type": "plus", "resets_at": 4102444800, "message": "quota exceeded: you hit your usage limit for plus"}}"#,
                ),
            )
            .mount(&server)
            .await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| options.stream.max_retries = Some(3));
        let events = collect(&server, &model, &ctx, &options).await;
        let error = error_of(&events);
        let message = error.error_message.unwrap_or_default();
        assert!(
            message.starts_with("You have hit your ChatGPT usage limit (plus plan)."),
            "got: {message}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    /// Upstream's catch-all path retries even non-retryable statuses while
    /// attempts remain (their messages never contain "usage limit").
    #[tokio::test]
    async fn non_retryable_401_still_exhausts_retries() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/codex/responses"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| options.stream.max_retries = Some(1));
        let events = collect(&server, &model, &ctx, &options).await;
        let error = error_of(&events);
        assert_eq!(error.error_message.as_deref(), Some("bad key"));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn retry_disabled_by_default() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/codex/responses"))
            .respond_with(
                wiremock::ResponseTemplate::new(429)
                    .insert_header("retry-after-ms", "1")
                    .set_body_string("rate limited"),
            )
            .mount(&server)
            .await;
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let events = collect(&server, &model, &ctx, &sse_options(|_| {})).await;
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Error { .. })
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    /// Oracle "zstd-compresses SSE request bodies": both a large and a tiny
    /// body ship compressed and decode to the request JSON.
    #[tokio::test]
    async fn zstd_compresses_sse_request_bodies() {
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let model = model_on(&server);
        let large_text = "compress me ".repeat(400);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg(&large_text)],
        );
        let (_, headers, events) = capture(&server, &model, &ctx, &sse_options(|_| {})).await;
        assert_eq!(header_value(&headers, "content-encoding"), Some("zstd"));
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));

        let requests = server.received_requests().await.unwrap();
        let decoded: Value =
            serde_json::from_slice(&zstd::stream::decode_all(&requests[0].body[..]).unwrap())
                .unwrap();
        assert_eq!(decoded["input"][0]["content"][0]["text"], json!(large_text));

        let ctx = ctx_with(Some("You are a helpful assistant."), vec![user_msg("hi")]);
        let (_, headers, _) = capture(&server, &model, &ctx, &sse_options(|_| {})).await;
        assert_eq!(header_value(&headers, "content-encoding"), Some("zstd"));
    }

    // ---- 7. websocket transport ----

    use std::sync::Mutex as StdMutex;

    /// Shared log for the mock websocket factory.
    #[derive(Default)]
    struct WsLog {
        connections: AtomicU32,
        closes: AtomicU32,
        sent_bodies: StdMutex<Vec<Value>>,
        connected_headers: StdMutex<Vec<Vec<(String, String)>>>,
    }

    struct MockConfig {
        /// Events dispatched after each `send`, keyed by (connection index,
        /// send index on that connection) — the oracle mocks dispatch from
        /// `send()`.
        per_send: Box<dyn Fn(u32, u32) -> Vec<WsEventKind> + Send + Sync>,
        /// Whether the connector future ever resolves (true = hang, for the
        /// connect-timeout test).
        connect_hangs: bool,
    }

    impl MockConfig {
        /// A socket that opens, swallows sends, and never answers.
        fn silent() -> Arc<MockConfig> {
            Arc::new(MockConfig {
                per_send: Box::new(|_, _| Vec::new()),
                connect_hangs: false,
            })
        }
    }

    fn mock_connector(log: Arc<WsLog>, config: Arc<MockConfig>) -> Arc<dyn websocket::WsConnector> {
        struct MockConnector(Arc<WsLog>, Arc<MockConfig>);
        impl websocket::WsConnector for MockConnector {
            fn connect(
                &self,
                _url: String,
                headers: Vec<(String, String)>,
            ) -> futures::future::BoxFuture<'static, Result<websocket::WsConnection, String>>
            {
                let log = self.0.clone();
                let config = self.1.clone();
                Box::pin(async move {
                    if config.connect_hangs {
                        std::future::pending::<()>().await;
                    }
                    let connection_index = log.connections.fetch_add(1, Ordering::SeqCst) + 1;
                    log.connected_headers.lock().unwrap().push(headers);
                    let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<websocket::WsOutgoing>(16);
                    let (incoming_tx, incoming_rx) = mpsc::channel::<websocket::WsEvent>(64);
                    let ready_state = Arc::new(std::sync::atomic::AtomicU8::new(1));
                    let pump_ready = ready_state.clone();
                    tokio::spawn(async move {
                        let mut send_index: u32 = 0;
                        while let Some(command) = outgoing_rx.recv().await {
                            match command {
                                websocket::WsOutgoing::Send(text) => {
                                    log.sent_bodies
                                        .lock()
                                        .unwrap()
                                        .push(serde_json::from_str(&text).unwrap());
                                    send_index += 1;
                                    for kind in (config.per_send)(connection_index, send_index) {
                                        let _ = incoming_tx.send(websocket::WsEvent { kind }).await;
                                    }
                                }
                                websocket::WsOutgoing::Close { .. } => {
                                    log.closes.fetch_add(1, Ordering::SeqCst);
                                    pump_ready.store(3, Ordering::SeqCst);
                                }
                            }
                        }
                    });
                    Ok(websocket::WsConnection {
                        outgoing: outgoing_tx,
                        incoming: incoming_rx,
                        ready_state,
                    })
                })
            }
        }
        Arc::new(MockConnector(log, config))
    }

    /// Serialized setup/teardown around the process-global websocket state.
    struct WsGuard(#[allow(dead_code)] tokio::sync::MutexGuard<'static, ()>);

    impl WsGuard {
        async fn acquire() -> Self {
            let guard = websocket::lock_global_state_for_tests().await;
            websocket::reset_websocket_state(None);
            websocket::close_websocket_sessions(None);
            websocket::set_clock_for_tests(0);
            WsGuard(guard)
        }
    }

    impl Drop for WsGuard {
        fn drop(&mut self) {
            websocket::set_connector_for_tests(None);
            websocket::set_clock_for_tests(0);
            websocket::reset_websocket_state(None);
            websocket::close_websocket_sessions(None);
        }
    }

    fn ws_frame(value: serde_json::Value) -> WsEventKind {
        WsEventKind::Text(value.to_string())
    }

    /// A terminal `response.completed` with `end_turn: false`, full text, and
    /// usage (the oracle's auto-transport fixture).
    fn ws_completed_events(response_id: &str) -> Vec<WsEventKind> {
        vec![
            ws_frame(json!({"type": "response.created", "response": {"id": response_id}})),
            ws_frame(json!({
                "type": "response.output_item.added",
                "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []},
            })),
            ws_frame(json!({"type": "response.output_text.delta", "delta": "Hello"})),
            ws_frame(json!({
                "type": "response.output_item.done",
                "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "Hello"}]},
            })),
            ws_frame(json!({
                "type": "response.completed",
                "response": {"id": response_id, "status": "completed", "end_turn": false, "usage": {"input_tokens": 5, "output_tokens": 3, "total_tokens": 8, "input_tokens_details": {"cached_tokens": 0}}},
            })),
        ]
    }

    /// A minimal terminal frame (the oracle's short fixtures).
    fn ws_completed_only(response_id: &str) -> Vec<WsEventKind> {
        vec![ws_frame(json!({
            "type": "response.completed",
            "response": {"id": response_id, "status": "completed", "usage": {"input_tokens": 5, "output_tokens": 3, "total_tokens": 8}},
        }))]
    }

    /// A full assistant turn (created / added / done / completed) with text.
    fn ws_turn(response_id: &str, text: &str) -> Vec<WsEventKind> {
        vec![
            ws_frame(json!({"type": "response.created", "response": {"id": response_id}})),
            ws_frame(json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []},
            })),
            ws_frame(json!({
                "type": "response.output_item.done",
                "output_index": 0,
                "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": text}]},
            })),
            ws_frame(json!({
                "type": "response.completed",
                "response": {"id": response_id, "status": "completed", "usage": {"input_tokens": 5, "output_tokens": 3, "total_tokens": 8}},
            })),
        ]
    }

    /// Oracle "forwards auto transport from streamSimple options and uses
    /// cached websocket context".
    #[tokio::test]
    async fn stream_simple_auto_transport_uses_cached_websocket() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        let log = Arc::new(WsLog::default());
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(|_, _| ws_completed_events("resp_1")),
                connect_hangs: false,
            }),
        )));
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.transport = None;
            options.stream.session_id = Some("session-auto".to_string());
        });
        let events = collect(&server, &model, &ctx, &options).await;
        let message = done_message(&events);
        assert_eq!(message.end_turn, Some(false));

        {
            let sent = log.sent_bodies.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0]["type"], json!("response.create"));
            assert_eq!(sent[0]["prompt_cache_key"], json!("session-auto"));
            let headers = log.connected_headers.lock().unwrap();
            let flat: BTreeMap<String, String> = headers[0].iter().cloned().collect();
            assert_eq!(
                flat.get("session-id").map(String::as_str),
                Some("session-auto")
            );
            assert_eq!(
                flat.get("x-client-request-id").map(String::as_str),
                Some("session-auto")
            );
            assert_eq!(
                flat.get("openai-beta").map(String::as_str),
                Some("responses_websockets=2026-02-06")
            );
            assert_eq!(
                flat.get("authorization").map(String::as_str),
                Some(format!("Bearer {}", mock_token("acc_test")).as_str())
            );
            drop(sent);
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        let stats = websocket::websocket_debug_stats("session-auto").unwrap();
        assert_eq!(stats.requests, 1);
        assert_eq!(stats.connections_created, 1);
        assert_eq!(stats.connections_reused, 0);
        assert_eq!(stats.cached_context_requests, 1);
        assert_eq!(stats.full_context_requests, 1);
        drop(guard);
    }

    /// A cancellation mid websocket stream settles the catch block's
    /// `"aborted"` branch with `"Request was aborted"` (upstream lines
    /// 330-340 + 484-494): the abort skips both retry arms, breaks the
    /// message read, and never falls back to SSE.
    #[tokio::test]
    async fn websocket_mid_stream_cancellation_settles_the_stream_aborted() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        let log = Arc::new(WsLog::default());
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                // The protocol prefix plus one delta, then silence: only the
                // abort path can end the stream.
                per_send: Box::new(|_, send_index| {
                    if send_index == 1 {
                        vec![
                            ws_frame(
                                json!({"type": "response.created", "response": {"id": "resp_1"}}),
                            ),
                            ws_frame(json!({
                                "type": "response.output_item.added",
                                "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []},
                            })),
                            ws_frame(
                                json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": ""}}),
                            ),
                            ws_frame(json!({
                                "type": "response.output_text.delta",
                                "delta": "Hello",
                            })),
                        ]
                    } else {
                        Vec::new()
                    }
                }),
                connect_hangs: false,
            }),
        )));
        let model = model_on(&server);
        let ctx = ctx_with(Some("You are helpful."), vec![user_msg("Say hello")]);
        let token = CancellationToken::new();
        let options = sse_options(|options| {
            options.stream.transport = Some(Transport::Websocket);
            options.stream.signal = Some(token.clone());
        });

        let api = OpenAiCodexResponses;
        let mut rx = api.stream_simple(&cfg(), &model, &ctx, &options);
        let mut deltas: Vec<String> = Vec::new();
        while deltas.is_empty() {
            let next = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
                .await
                .expect("timed out waiting for first delta");
            match next {
                Some(AssistantMessageEvent::Start { .. })
                | Some(AssistantMessageEvent::TextStart { .. }) => {}
                Some(AssistantMessageEvent::TextDelta { delta, .. }) => deltas.push(delta),
                Some(other) => panic!("unexpected event before delta: {other:?}"),
                None => panic!("stream ended before any delta"),
            }
        }
        token.cancel();
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .expect("timed out waiting for terminal event after abort")
            .expect("terminal event");
        match terminal {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
            }
            other => panic!("expected terminal error event, got {other:?}"),
        }
        // No SSE fallback request was issued.
        assert!(server.received_requests().await.unwrap().is_empty());
        drop(guard);
    }

    /// Oracle "scopes cached websockets to the authenticated account" (issue
    /// #7284): rotating accounts never reuse another account's socket.
    #[tokio::test]
    async fn scopes_cached_websockets_to_the_authenticated_account() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        let log = Arc::new(WsLog::default());
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(move |_, _| {
                    let id = counter_clone.fetch_add(1, Ordering::SeqCst) + 1;
                    ws_completed_only(&format!("resp_{id}"))
                }),
                connect_hangs: false,
            }),
        )));
        let model = model_on(&server);
        let ctx = ctx_with(None, vec![]);
        for account in ["account-a", "account-b", "account-a"] {
            let options = SimpleStreamOptions {
                stream: StreamOptions {
                    api_key: Some(mock_token(account)),
                    session_id: Some("shared-session".to_string()),
                    transport: Some(Transport::WebsocketCached),
                    ..Default::default()
                },
                ..SimpleStreamOptions::default()
            };
            let events = collect(&server, &model, &ctx, &options).await;
            assert!(
                matches!(events.last(), Some(AssistantMessageEvent::Done { .. })),
                "{events:?}"
            );
        }
        {
            let headers = log.connected_headers.lock().unwrap();
            let accounts: Vec<String> = headers
                .iter()
                .map(|headers| {
                    headers
                        .iter()
                        .find(|(name, _)| name == "chatgpt-account-id")
                        .unwrap()
                        .1
                        .clone()
                })
                .collect();
            assert_eq!(accounts, ["account-a", "account-b"]);
            let authorizations: Vec<String> = headers
                .iter()
                .map(|headers| {
                    headers
                        .iter()
                        .find(|(name, _)| name == "authorization")
                        .unwrap()
                        .1
                        .clone()
                })
                .collect();
            assert_eq!(
                authorizations,
                [
                    format!("Bearer {}", mock_token("account-a")),
                    format!("Bearer {}", mock_token("account-b"))
                ]
            );
            drop(headers);
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        let stats = websocket::websocket_debug_stats("shared-session").unwrap();
        assert_eq!(stats.requests, 3);
        assert_eq!(stats.connections_created, 2);
        assert_eq!(stats.connections_reused, 1);
        drop(guard);
    }

    /// Oracle "closes one-shot websockets when cacheRetention is none".
    #[tokio::test]
    async fn closes_one_shot_websockets_when_cache_retention_is_none() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        let log = Arc::new(WsLog::default());
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(move |_, _| {
                    let id = counter_clone.fetch_add(1, Ordering::SeqCst) + 1;
                    ws_completed_only(&format!("resp_{id}"))
                }),
                connect_hangs: false,
            }),
        )));
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.transport = None;
            options.stream.session_id = Some("one-off-summary".to_string());
            options.stream.cache_retention = Some(CacheRetention::None);
        });
        for _ in 0..2 {
            let events = collect(&server, &model, &ctx, &options).await;
            assert!(matches!(
                events.last(),
                Some(AssistantMessageEvent::Done { .. })
            ));
        }
        assert_eq!(log.connections.load(Ordering::SeqCst), 2);
        assert_eq!(log.closes.load(Ordering::SeqCst), 2);
        {
            let sent = log.sent_bodies.lock().unwrap();
            assert_eq!(sent.len(), 2);
            assert!(sent
                .iter()
                .all(|body| body.get("prompt_cache_key").is_none()));
            drop(sent);
        }
        assert!(websocket::websocket_debug_stats("one-off-summary").is_none());
        assert!(server.received_requests().await.unwrap().is_empty());
        drop(guard);
    }

    /// Oracle "falls back to SSE when websocket connect does not open before
    /// the connect timeout".
    #[tokio::test]
    async fn falls_back_to_sse_on_connect_timeout() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let log = Arc::new(WsLog::default());
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(|_, _| Vec::new()),
                connect_hangs: true,
            }),
        )));
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.transport = None;
            options.stream.session_id = Some("ws-connect-timeout".to_string());
            options.stream.timeout_ms = Some(300_000);
            options.stream.websocket_connect_timeout_ms = Some(50);
        });
        let events = collect(&server, &model, &ctx, &options).await;
        let message = done_message(&events);
        match message.content.first() {
            Some(AssistantBlock::Text(text)) => assert_eq!(text.text, "Hello"),
            other => panic!("expected text block, got {other:?}"),
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let stats = websocket::websocket_debug_stats("ws-connect-timeout").unwrap();
        assert_eq!(stats.requests, 0);
        assert_eq!(stats.websocket_failures, 1);
        assert_eq!(stats.sse_fallbacks, 1);
        assert_eq!(stats.websocket_fallback_active, Some(true));
        assert_eq!(
            stats.last_websocket_error.as_deref(),
            Some("WebSocket connect timeout after 50ms")
        );
        drop(guard);
    }

    /// Oracle "reconnects once when the websocket connection limit is reached
    /// before output starts".
    #[tokio::test]
    async fn reconnects_once_on_connection_limit_before_start() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        let log = Arc::new(WsLog::default());
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(|connection, _| {
                    if connection == 1 {
                        vec![ws_frame(json!({
                            "type": "error",
                            "error": {"code": "websocket_connection_limit_reached"},
                        }))]
                    } else {
                        ws_completed_only("resp_1")
                    }
                }),
                connect_hangs: false,
            }),
        )));
        let model = model_on(&server);
        let ctx = ctx_with(None, vec![]);
        let options = sse_options(|options| options.stream.transport = None);
        let events = collect(&server, &model, &ctx, &options).await;
        let message = done_message(&events);
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(log.connections.load(Ordering::SeqCst), 2);
        assert!(server.received_requests().await.unwrap().is_empty());
        drop(guard);
    }

    /// Oracle "falls back to SSE when a websocket is idle before the first
    /// event": `timeoutMs` doubles as the inter-message idle timeout.
    #[tokio::test]
    async fn falls_back_to_sse_when_idle_before_first_event() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let log = Arc::new(WsLog::default());
        websocket::set_connector_for_tests(Some(mock_connector(log.clone(), MockConfig::silent())));
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.transport = None;
            options.stream.session_id = Some("ws-idle-before-start".to_string());
            options.stream.timeout_ms = Some(50);
        });
        let events = collect(&server, &model, &ctx, &options).await;
        let message = done_message(&events);
        match message.content.first() {
            Some(AssistantBlock::Text(text)) => assert_eq!(text.text, "Hello"),
            other => panic!("expected text block, got {other:?}"),
        }
        assert_eq!(log.sent_bodies.lock().unwrap().len(), 1);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let stats = websocket::websocket_debug_stats("ws-idle-before-start").unwrap();
        assert_eq!(stats.websocket_failures, 1);
        assert_eq!(stats.sse_fallbacks, 1);
        assert_eq!(stats.websocket_fallback_active, Some(true));
        assert_eq!(
            stats.last_websocket_error.as_deref(),
            Some("WebSocket idle timeout after 50ms")
        );
        drop(guard);
    }

    /// Oracle "errors when a websocket is idle after the stream started":
    /// no SSE fallback once events flowed.
    #[tokio::test]
    async fn errors_when_idle_after_stream_started() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        let log = Arc::new(WsLog::default());
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(|_, _| {
                    vec![ws_frame(json!({
                        "type": "response.output_item.added",
                        "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []},
                    }))]
                }),
                connect_hangs: false,
            }),
        )));
        let model = model_on(&server);
        let ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.transport = None;
            options.stream.timeout_ms = Some(50);
        });
        let events = collect(&server, &model, &ctx, &options).await;
        let error = error_of(&events);
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("WebSocket idle timeout after 50ms")
        );
        assert!(server.received_requests().await.unwrap().is_empty());
        drop(guard);
    }

    /// Oracle "opens a fresh cached websocket before the backend connection
    /// age limit": a cached socket older than 55 minutes is replaced.
    #[tokio::test]
    async fn opens_fresh_socket_past_connection_age_limit() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        let log = Arc::new(WsLog::default());
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(move |_, _| {
                    let id = counter_clone.fetch_add(1, Ordering::SeqCst) + 1;
                    ws_completed_only(&format!("resp_{id}"))
                }),
                connect_hangs: false,
            }),
        )));
        websocket::set_clock_for_tests(1_783_036_800_000); // 2026-07-03T00:00:00Z
        let model = model_on(&server);
        let first_ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.transport = Some(Transport::WebsocketCached);
            options.stream.session_id = Some("aged-ws-session".to_string());
        });
        let first = collect(&server, &model, &first_ctx, &options).await;
        let first_message = done_message(&first);

        websocket::set_clock_for_tests(1_783_036_800_000 + 56 * 60 * 1000);
        let second_ctx = normalize_context(&Context {
            system_prompt: None,
            messages: [
                first_ctx.messages().to_vec(),
                vec![Message::Assistant(first_message), user_msg("Now finish")],
            ]
            .concat(),
            tools: None,
        });
        let second = collect(&server, &model, &second_ctx, &options).await;
        assert!(matches!(
            second.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));

        assert_eq!(log.connections.load(Ordering::SeqCst), 2);
        {
            let sent = log.sent_bodies.lock().unwrap();
            assert_eq!(sent.len(), 2);
            drop(sent);
        }
        let stats = websocket::websocket_debug_stats("aged-ws-session").unwrap();
        assert_eq!(stats.requests, 2);
        assert_eq!(stats.connections_created, 2);
        assert_eq!(stats.connections_reused, 0);
        assert_eq!(stats.cached_context_requests, 2);
        drop(guard);
    }

    /// Oracle "sends only response input deltas in websocket-cached mode":
    /// the second request carries `previous_response_id` plus the delta
    /// suffix, and `store` stays false.
    #[tokio::test]
    async fn sends_only_input_deltas_in_websocket_cached_mode() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        let log = Arc::new(WsLog::default());
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(|_, send| {
                    if send == 1 {
                        vec![
                            ws_frame(json!({
                                "type": "response.output_item.added",
                                "item": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_1", "name": "sample_tool", "input": ""},
                            })),
                            ws_frame(
                                json!({"type": "response.custom_tool_call_input.delta", "item_id": "ctc_1", "delta": "abc"}),
                            ),
                            ws_frame(
                                json!({"type": "response.custom_tool_call_input.done", "item_id": "ctc_1", "input": "abc"}),
                            ),
                            ws_frame(json!({
                                "type": "response.output_item.done",
                                "item": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_1", "name": "sample_tool", "input": "abc"},
                            })),
                            ws_frame(json!({
                                "type": "response.completed",
                                "response": {"id": "resp_1", "status": "completed", "usage": {"input_tokens": 5, "output_tokens": 3, "total_tokens": 8, "input_tokens_details": {"cached_tokens": 0}}},
                            })),
                        ]
                    } else {
                        ws_completed_only("resp_2")
                    }
                }),
                connect_hangs: false,
            }),
        )));
        let mut model = model_on(&server);
        model.compat = Some(json!({"supportsOpenAIGrammarTools": true}));
        let grammar_tool = Tool {
            name: "sample_tool".into(),
            description: "Sample tool".into(),
            parameters: json!({"type": "object", "properties": {"payload": {"type": "string"}}, "required": ["payload"]}),
            constrained_sampling: Some(ConstrainedSampling::Grammar(GrammarSampling {
                variants: BTreeMap::from([(
                    crate::ai::types::tool::GrammarFormat::Lark,
                    "start: /[a-z]+/".to_string(),
                )]),
            })),
        };
        let first_context = Context {
            system_prompt: Some("You are a helpful assistant.".to_string()),
            messages: vec![user_msg("Use the tool")],
            tools: Some(vec![grammar_tool]),
        };
        let first_ctx = normalize_context(&first_context);
        let options = sse_options(|options| {
            options.stream.transport = Some(Transport::WebsocketCached);
            options.stream.session_id = Some("session-1".to_string());
        });
        let first = collect(&server, &model, &first_ctx, &options).await;
        let first_message = done_message(&first);

        let second_ctx = normalize_context(&Context {
            system_prompt: first_context.system_prompt.clone(),
            messages: [
                first_context.messages.clone(),
                vec![
                    Message::Assistant(first_message),
                    Message::ToolResult(crate::ai::types::message::ToolResultMessage {
                        tool_call_id: "call_1|ctc_1".to_string(),
                        tool_name: "sample_tool".to_string(),
                        content: vec![TextOrImageBlock::Text(
                            crate::ai::types::content::TextContent {
                                text: "real result".to_string(),
                                text_signature: None,
                            },
                        )],
                        details: None,
                        usage: None,
                        is_error: false,
                        timestamp: 2,
                    }),
                    user_msg("Now finish"),
                ],
            ]
            .concat(),
            // The oracle spreads firstContext, carrying the grammar tool
            // declaration into the second request.
            tools: first_context.tools.clone(),
        });
        let second = collect(&server, &model, &second_ctx, &options).await;
        assert!(matches!(
            second.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));

        {
            let sent = log.sent_bodies.lock().unwrap();
            assert_eq!(sent.len(), 2);
            assert_eq!(sent[0]["store"], json!(false));
            assert!(sent[0].get("previous_response_id").is_none());
            assert_eq!(
                sent[0]["input"],
                json!([{"role": "user", "content": [{"type": "input_text", "text": "Use the tool"}]}])
            );
            assert_eq!(sent[1]["store"], json!(false));
            assert_eq!(sent[1]["previous_response_id"], json!("resp_1"));
            assert_eq!(
                sent[1]["input"],
                json!([
                    {"type": "custom_tool_call_output", "call_id": "call_1", "output": "real result"},
                    {"role": "user", "content": [{"type": "input_text", "text": "Now finish"}]},
                ])
            );
            drop(sent);
        }
        let stats = websocket::websocket_debug_stats("session-1").unwrap();
        assert_eq!(stats.requests, 2);
        assert_eq!(stats.connections_created, 1);
        assert_eq!(stats.connections_reused, 1);
        assert_eq!(stats.cached_context_requests, 2);
        assert_eq!(stats.store_true_requests, 0);
        assert_eq!(stats.full_context_requests, 1);
        assert_eq!(stats.delta_requests, 1);
        assert_eq!(stats.last_delta_input_items, Some(2));
        assert_eq!(stats.last_previous_response_id.as_deref(), Some("resp_1"));
        drop(guard);
    }

    /// Oracle "recovers a missing cached websocket continuation via
    /// websocket": `previous_response_not_found` reconnects once with the
    /// full input on a fresh socket.
    #[tokio::test]
    async fn recovers_missing_continuation_via_websocket() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        let log = Arc::new(WsLog::default());
        let sends = Arc::new(AtomicU32::new(0));
        let sends_clone = sends.clone();
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(move |_, _| {
                    let send = sends_clone.fetch_add(1, Ordering::SeqCst) + 1;
                    if send == 2 {
                        return vec![
                            ws_frame(json!({"type": "codex.rate_limits", "plan_type": "plus"})),
                            ws_frame(json!({
                                "type": "error",
                                "status": 400,
                                "error": {
                                    "code": "previous_response_not_found",
                                    "message": "Previous response with id 'resp_1' not found.",
                                    "param": "previous_response_id",
                                },
                            })),
                        ];
                    }
                    if send == 1 {
                        ws_turn("resp_1", "Hello")
                    } else {
                        ws_turn("resp_2", "Recovered")
                    }
                }),
                connect_hangs: false,
            }),
        )));
        let model = model_on(&server);
        let first_ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.transport = Some(Transport::WebsocketCached);
            options.stream.session_id = Some("missing-continuation-websocket".to_string());
        });
        let first = collect(&server, &model, &first_ctx, &options).await;
        let first_message = done_message(&first);

        let second_ctx = normalize_context(&Context {
            system_prompt: None,
            messages: [
                first_ctx.messages().to_vec(),
                vec![Message::Assistant(first_message), user_msg("Now finish")],
            ]
            .concat(),
            tools: None,
        });
        let events = collect(&server, &model, &second_ctx, &options).await;
        assert!(!event_types(&events).contains(&"error"));
        // Exactly one start across the whole request (the retried attempt
        // emits none because the failed one produced no events).
        assert_eq!(
            event_types(&events)
                .iter()
                .filter(|t| **t == "start")
                .count(),
            1
        );
        let second = done_message(&events);
        assert_eq!(second.stop_reason, StopReason::Stop);
        match second.content.first() {
            Some(AssistantBlock::Text(text)) => assert_eq!(text.text, "Recovered"),
            other => panic!("expected text block, got {other:?}"),
        }

        assert_eq!(log.connections.load(Ordering::SeqCst), 2);
        {
            let sent = log.sent_bodies.lock().unwrap();
            assert_eq!(sent.len(), 3);
            assert_eq!(sent[1]["previous_response_id"], json!("resp_1"));
            assert_eq!(
                sent[1]["input"],
                json!([{"role": "user", "content": [{"type": "input_text", "text": "Now finish"}]}])
            );
            assert!(sent[2].get("previous_response_id").is_none());
            assert_eq!(sent[2]["input"].as_array().unwrap().len(), 3);
            assert_eq!(
                sent[2]["input"].as_array().unwrap().last().unwrap(),
                &json!({"role": "user", "content": [{"type": "input_text", "text": "Now finish"}]})
            );
            drop(sent);
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        let stats = websocket::websocket_debug_stats("missing-continuation-websocket").unwrap();
        assert_eq!(stats.requests, 3);
        assert_eq!(stats.connections_created, 2);
        assert_eq!(stats.connections_reused, 1);
        assert_eq!(stats.full_context_requests, 2);
        assert_eq!(stats.delta_requests, 1);
        assert_eq!(stats.websocket_failures, 0);
        assert_eq!(stats.sse_fallbacks, 0);
        drop(guard);
    }

    /// Oracle "recovers a missing cached websocket continuation via sse":
    /// the websocket retry fails again (socket error before start), so the
    /// request falls back to SSE.
    #[tokio::test]
    async fn recovers_missing_continuation_via_sse() {
        let guard = WsGuard::acquire().await;
        let server = wiremock::MockServer::start().await;
        mount_codex(&server, completed_sse()).await;
        let log = Arc::new(WsLog::default());
        let sends = Arc::new(AtomicU32::new(0));
        let sends_clone = sends.clone();
        websocket::set_connector_for_tests(Some(mock_connector(
            log.clone(),
            Arc::new(MockConfig {
                per_send: Box::new(move |_, _| {
                    // Turn 1 succeeds, the reused socket hits a missing
                    // continuation, and the reconnect dies with a socket
                    // error before any event — SSE fallback takes over.
                    let send = sends_clone.fetch_add(1, Ordering::SeqCst) + 1;
                    if send == 1 {
                        ws_turn("resp_1", "Hello")
                    } else if send == 2 {
                        vec![ws_frame(json!({
                            "type": "error",
                            "error": {"code": "previous_response_not_found", "message": "Previous response with id 'resp_1' not found."},
                        }))]
                    } else {
                        vec![WsEventKind::Error("retry websocket failed".to_string())]
                    }
                }),
                connect_hangs: false,
            }),
        )));
        let model = model_on(&server);
        let first_ctx = ctx_with(
            Some("You are a helpful assistant."),
            vec![user_msg("Say hello")],
        );
        let options = sse_options(|options| {
            options.stream.transport = Some(Transport::WebsocketCached);
            options.stream.session_id = Some("missing-continuation-sse".to_string());
        });
        let first = collect(&server, &model, &first_ctx, &options).await;
        let first_message = done_message(&first);

        let second_ctx = normalize_context(&Context {
            system_prompt: None,
            messages: [
                first_ctx.messages().to_vec(),
                vec![Message::Assistant(first_message), user_msg("Now finish")],
            ]
            .concat(),
            tools: None,
        });
        let events = collect(&server, &model, &second_ctx, &options).await;
        assert!(!event_types(&events).contains(&"error"));
        assert_eq!(
            event_types(&events)
                .iter()
                .filter(|t| **t == "start")
                .count(),
            1
        );
        let second = done_message(&events);
        match second.content.first() {
            Some(AssistantBlock::Text(text)) => assert_eq!(text.text, "Hello"),
            other => panic!("expected text block, got {other:?}"),
        }

        assert_eq!(log.connections.load(Ordering::SeqCst), 2);
        {
            let sent = log.sent_bodies.lock().unwrap();
            assert_eq!(sent.len(), 3);
            drop(sent);
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let stats = websocket::websocket_debug_stats("missing-continuation-sse").unwrap();
        assert_eq!(stats.requests, 3);
        assert_eq!(stats.connections_created, 2);
        assert_eq!(stats.connections_reused, 1);
        assert_eq!(stats.full_context_requests, 2);
        assert_eq!(stats.delta_requests, 1);
        assert_eq!(stats.websocket_failures, 1);
        assert_eq!(stats.sse_fallbacks, 1);
        drop(guard);
    }

    // ---- 8. pure helpers ----

    #[test]
    fn resolve_codex_url_cases() {
        assert_eq!(
            resolve_codex_url(Some("https://chatgpt.com/backend-api")),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_codex_url(None),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_codex_url(Some("   ")),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_codex_url(Some("https://proxy.example/api/")),
            "https://proxy.example/api/codex/responses"
        );
        assert_eq!(
            resolve_codex_url(Some("https://proxy.example/backend-api/codex")),
            "https://proxy.example/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_codex_url(Some("https://proxy.example/codex/responses/")),
            "https://proxy.example/codex/responses"
        );
    }

    #[test]
    fn resolve_codex_websocket_url_swaps_scheme() {
        assert_eq!(
            resolve_codex_websocket_url(Some("https://chatgpt.com/backend-api")),
            "wss://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_codex_websocket_url(Some("http://localhost:8080/backend-api")),
            "ws://localhost:8080/backend-api/codex/responses"
        );
    }

    #[test]
    fn extract_account_id_validates_jwt() {
        assert_eq!(extract_account_id(&mock_token("acc_x")).unwrap(), "acc_x");
        for bad in ["", "aaa.bbb", "not a jwt", "aaa.cGF5bG9hZA.bbb"] {
            assert_eq!(
                extract_account_id(bad).unwrap_err().message,
                "Failed to extract accountId from token",
                "{bad}"
            );
        }
    }

    #[test]
    fn decode_standard_base64_round_trips() {
        for input in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            let encoded = tests::base64_encode(input.as_bytes());
            let decoded = decode_standard_base64(&encoded).unwrap();
            assert_eq!(String::from_utf8(decoded).unwrap(), input, "{encoded}");
        }
        assert!(decode_standard_base64("a-b_c!").is_none());
    }

    #[test]
    fn is_retryable_error_table() {
        assert!(!is_retryable_error(429, "GoUsageLimitError: monthly cap"));
        assert!(!is_retryable_error(429, "insufficient_quota"));
        assert!(is_retryable_error(429, "slow down"));
        assert!(is_retryable_error(500, ""));
        assert!(is_retryable_error(502, ""));
        assert!(is_retryable_error(503, ""));
        assert!(is_retryable_error(504, ""));
        assert!(is_retryable_error(400, "RateLimit hit"));
        assert!(is_retryable_error(400, "service unavailable"));
        assert!(is_retryable_error(400, "upstream connect error"));
        assert!(is_retryable_error(400, "connection refused"));
        assert!(!is_retryable_error(400, "bad request"));
        assert!(!is_retryable_error(401, "unauthorized"));
    }

    #[test]
    fn get_retry_after_delay_ms_forms() {
        let headers = |values: &[(&str, &str)]| {
            let mut map = reqwest::header::HeaderMap::new();
            for (name, value) in values {
                map.insert(
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                    reqwest::header::HeaderValue::from_str(value).unwrap(),
                );
            }
            map
        };
        let now = 1_778_630_400_000; // 2026-05-13T00:00:00Z
        assert_eq!(
            get_retry_after_delay_ms(&headers(&[("retry-after-ms", "1500")]), now),
            Some(1500)
        );
        assert_eq!(
            get_retry_after_delay_ms(&headers(&[("retry-after", "60")]), now),
            Some(60_000)
        );
        assert_eq!(
            get_retry_after_delay_ms(
                &headers(&[("retry-after", "Wed, 13 May 2026 00:00:45 GMT")]),
                now
            ),
            Some(45_000)
        );
        assert_eq!(
            get_retry_after_delay_ms(&headers(&[("retry-after", "not a date")]), now),
            None
        );
        assert_eq!(get_retry_after_delay_ms(&headers(&[]), now), None);
    }

    #[test]
    fn validate_retry_delay_ms_message_is_exact() {
        assert_eq!(validate_retry_delay_ms(1000, Some(1000)).unwrap(), 1000);
        let error = validate_retry_delay_ms(2000, Some(1000)).unwrap_err();
        assert_eq!(error.message, "Server requested 2s retry delay (max: 1s)");
        assert_eq!(error.kind, CodexStreamErrorKind::RetryDelayExceeded);
        // Default cap 60s; 0 disables.
        assert!(validate_retry_delay_ms(61_000, None).is_err());
        assert_eq!(validate_retry_delay_ms(61_000, Some(0)).unwrap(), 61_000);
    }

    #[test]
    fn base_delay_ms_doubles() {
        assert_eq!(base_delay_ms(0), 1000);
        assert_eq!(base_delay_ms(1), 2000);
        assert_eq!(base_delay_ms(2), 4000);
    }

    #[test]
    fn parse_error_response_renders_usage_limit() {
        // The minutes text depends on the wall clock; assert the stable
        // prefix and the plan rendering instead.
        let parsed = parse_error_response(
            429,
            r#"{"error": {"code": "usage_limit_reached", "plan_type": "Pro", "resets_at": 0, "message": "limit"}}"#,
        );
        assert_eq!(
            parsed.friendly_message.as_deref(),
            Some("You have hit your ChatGPT usage limit (pro plan).")
        );
        assert_eq!(parsed.message, "limit");

        let non_json = parse_error_response(401, "bad key");
        assert!(non_json.friendly_message.is_none());
        assert_eq!(non_json.message, "bad key");

        // Non-429 with a usage code still renders friendly; 429 with another
        // code renders friendly too (status drives it).
        let status_only = parse_error_response(429, r#"{"error": {"code": "other"}}"#);
        assert!(status_only
            .friendly_message
            .as_deref()
            .unwrap_or_default()
            .starts_with("You have hit your ChatGPT usage limit."));
    }

    #[test]
    fn build_request_body_default_shape() {
        let model = model();
        let ctx = ctx_with(Some("Be terse."), vec![user_msg("hi")]);
        let body = build_request_body(
            &model,
            &ctx,
            &SimpleStreamOptions::default(),
            &CodexRequestOptions::default(),
            Some("sess"),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(body["model"], json!("gpt-5.1-codex"));
        assert_eq!(body["store"], json!(false));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["instructions"], json!("Be terse."));
        assert_eq!(body["text"], json!({"verbosity": "low"}));
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["prompt_cache_key"], json!("sess"));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert_eq!(body["reasoning"], json!({"effort": "none"}));
        assert!(body.get("temperature").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("service_tier").is_none());
    }

    #[test]
    fn build_request_body_defaults_instructions_when_no_system_prompt() {
        let model = model();
        let ctx = ctx_with(None, vec![user_msg("hi")]);
        let body = build_request_body(
            &model,
            &ctx,
            &SimpleStreamOptions::default(),
            &CodexRequestOptions::default(),
            None,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(body["instructions"], json!("You are a helpful assistant."));
    }

    #[test]
    fn build_request_body_reasoning_map_fallbacks() {
        let model = model_with_map(json!({"low": null}));
        let ctx = ctx_with(None, vec![user_msg("hi")]);
        let codex = CodexRequestOptions {
            reasoning_effort: Some("low".to_string()),
            ..Default::default()
        };
        let body = build_request_body(
            &model,
            &ctx,
            &SimpleStreamOptions::default(),
            &codex,
            None,
            &Default::default(),
        )
        .unwrap();
        // JS `thinkingLevelMap["low"] ?? "low"`: null falls back to the level.
        assert_eq!(
            body["reasoning"],
            json!({"effort": "low", "summary": "auto"})
        );

        // effort "none" with map.off = null omits reasoning entirely.
        let model = model_with_map(json!({"off": null}));
        let codex = CodexRequestOptions {
            reasoning_effort: Some("none".to_string()),
            ..Default::default()
        };
        let body = build_request_body(
            &model,
            &ctx,
            &SimpleStreamOptions::default(),
            &codex,
            None,
            &Default::default(),
        )
        .unwrap();
        assert!(body.get("reasoning").is_none());

        // effort "none" without a map sends "none".
        let model = Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            thinking_level_map: None,
            ..model
        };
        let body = build_request_body(
            &model,
            &ctx,
            &SimpleStreamOptions::default(),
            &CodexRequestOptions {
                reasoning_effort: Some("none".to_string()),
                ..Default::default()
            },
            None,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(
            body["reasoning"],
            json!({"effort": "none", "summary": "auto"})
        );
    }

    #[test]
    fn service_tier_helpers() {
        assert_eq!(
            get_service_tier_cost_multiplier("gpt-5.5", Some("flex")),
            0.5
        );
        assert_eq!(
            get_service_tier_cost_multiplier("gpt-5.5", Some("priority")),
            2.5
        );
        assert_eq!(
            get_service_tier_cost_multiplier("gpt-5.1-codex", Some("priority")),
            2.0
        );
        assert_eq!(get_service_tier_cost_multiplier("gpt-5.5", None), 1.0);
        assert_eq!(
            get_service_tier_cost_multiplier("gpt-5.5", Some("default")),
            1.0
        );

        assert_eq!(
            resolve_codex_service_tier(Some("default"), Some("flex")).as_deref(),
            Some("flex")
        );
        assert_eq!(
            resolve_codex_service_tier(Some("default"), Some("priority")).as_deref(),
            Some("priority")
        );
        assert_eq!(
            resolve_codex_service_tier(Some("flex"), None).as_deref(),
            Some("flex")
        );
        assert_eq!(
            resolve_codex_service_tier(None, Some("flex")).as_deref(),
            Some("flex")
        );
        assert_eq!(resolve_codex_service_tier(None, None), None);
    }

    #[test]
    fn uuid_v7_shape_and_uniqueness() {
        let first = uuid_v7();
        let second = uuid_v7();
        assert_ne!(first, second);
        let pattern = regex_like_check(&first);
        assert!(pattern, "{first}");
        assert_eq!(&first[14..15], "7", "version nibble: {first}");
        let variant = u8::from_str_radix(&first[19..20], 16).unwrap();
        assert!((0x8..=0xb).contains(&variant), "variant bits: {first}");
    }

    fn regex_like_check(value: &str) -> bool {
        let bytes = value.as_bytes();
        bytes.len() == 36
            && bytes[8] == b'-'
            && bytes[13] == b'-'
            && bytes[18] == b'-'
            && bytes[23] == b'-'
            && value.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
    }

    #[test]
    fn map_codex_event_classification() {
        let mut end_turn: Option<bool> = None;
        // Skip typeless events.
        assert!(matches!(
            map_codex_event(&json!({"foo": 1}), &mut end_turn),
            CodexMapped::Skip
        ));
        // Error event with nested code/message.
        match map_codex_event(
            &json!({"type": "error", "error": {"code": "boom", "message": "bad"}}),
            &mut end_turn,
        ) {
            CodexMapped::Error(error) => {
                assert_eq!(error.message, "Codex error: bad");
                assert!(error.is_non_transport());
            }
            other => panic!("expected error, got {other:?}"),
        }
        // response.failed.
        match map_codex_event(
            &json!({"type": "response.failed", "response": {"error": {"code": "x", "message": "m"}}}),
            &mut end_turn,
        ) {
            CodexMapped::Error(error) => assert_eq!(error.message, "m"),
            other => panic!("expected error, got {other:?}"),
        }
        // response.done normalizes to response.completed with end_turn and
        // an unknown status stripped.
        match map_codex_event(
            &json!({"type": "response.done", "response": {"status": "martian", "end_turn": true}}),
            &mut end_turn,
        ) {
            CodexMapped::Terminal(ResponsesStreamEvent::Completed { response }) => {
                assert_eq!(end_turn, Some(true));
                assert_eq!(response.status, None);
            }
            other => panic!("expected terminal, got {other:?}"),
        }
        // Known statuses survive.
        let mut end_turn = None;
        match map_codex_event(
            &json!({"type": "response.incomplete", "response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}}),
            &mut end_turn,
        ) {
            CodexMapped::Terminal(ResponsesStreamEvent::Completed { response }) => {
                assert_eq!(response.status.as_deref(), Some("incomplete"));
            }
            other => panic!("expected terminal, got {other:?}"),
        }
        // Other events pass through.
        assert!(matches!(
            map_codex_event(&json!({"type": "response.output_text.delta", "delta": "x"}), &mut end_turn),
            CodexMapped::Event(ResponsesStreamEvent::OutputTextDelta { delta, .. }) if delta == "x"
        ));
    }

    /// The websocket-cached delta computation pins the oracle's second-body
    /// shape at the pure level too (baseline prefix, mismatch, short input).
    #[test]
    fn websocket_delta_computation() {
        let baseline_input =
            json!([{"role": "user", "content": [{"type": "input_text", "text": "one"}]}]);
        let response_items = json!([
            {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_1", "name": "t", "input": "abc"}
        ]);
        let mut continuation = Some(websocket::ContinuationState {
            last_request_body: json!({"model": "m", "input": baseline_input, "store": false}),
            last_response_id: "resp_1".to_string(),
            last_response_items: response_items.as_array().unwrap().clone(),
        });

        let body = json!({
            "model": "m",
            "store": false,
            "input": [
                {"role": "user", "content": [{"type": "input_text", "text": "one"}]},
                {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_1", "name": "t", "input": "abc"},
                {"type": "custom_tool_call_output", "call_id": "call_1", "output": "ok"},
                {"role": "user", "content": [{"type": "input_text", "text": "two"}]},
            ],
        });
        let reduced = websocket::build_cached_websocket_request_body(&mut continuation, &body);
        assert_eq!(reduced["previous_response_id"], json!("resp_1"));
        assert_eq!(
            reduced["input"],
            json!([
                {"type": "custom_tool_call_output", "call_id": "call_1", "output": "ok"},
                {"role": "user", "content": [{"type": "input_text", "text": "two"}]},
            ])
        );

        // A different model field invalidates the continuation.
        let mut continuation = Some(websocket::ContinuationState {
            last_request_body: json!({"model": "m", "input": baseline_input.clone()}),
            last_response_id: "resp_1".to_string(),
            last_response_items: vec![],
        });
        let diverged = json!({"model": "other", "input": []});
        websocket::build_cached_websocket_request_body(&mut continuation, &diverged);
        assert!(continuation.is_none());

        // A shortened input clears the slot and falls back to the full body.
        let mut continuation = Some(websocket::ContinuationState {
            last_request_body: json!({"model": "m", "input": baseline_input}),
            last_response_id: "resp_1".to_string(),
            last_response_items: vec![json!({"type": "x"}), json!({"type": "y"})],
        });
        let short = json!({"model": "m", "input": []});
        let full = websocket::build_cached_websocket_request_body(&mut continuation, &short);
        assert!(full.get("previous_response_id").is_none());
        assert!(continuation.is_none());
    }
}
