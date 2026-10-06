//! Google Generative AI (Gemini Developer API) endpoint — full port of the
//! stream/streamSimple implementations from upstream
//! `packages/ai/src/api/google-generative-ai.ts` (470 lines): the
//! `generateContentStream` request body, the `alt=sse` wire protocol, the
//! streaming block state machine (text/thinking/toolCall), raw finish-reason
//! preservation, the usage/cost accounting, and the terminal framing.
//!
//! Wire shape: the port reproduces the pinned `@google/genai` SDK 2.21.0
//! request path directly (dist/node `index.cjs`):
//! - `POST {baseUrl}/models/{model}:streamGenerateContent?alt=sse` — the model
//!   name rides the URL path (`tModel`: `models/` prefix unless the id already
//!   starts with `models/`/`tunedModels/`; `..`/`?`/`&` reject with
//!   `invalid model parameter`), never the body.
//! - `model.baseUrl` set → it is the whole base (pi passes `apiVersion: ""` so
//!   no version segment is appended); empty → the SDK default
//!   `https://generativelanguage.googleapis.com/v1beta`.
//! - Auth is the `x-goog-api-key` header (the SDK's `NodeAuth.addKeyHeader`).
//! - Body (via `generateContentParametersToMldev`): `contents`, then
//!   `generationConfig` (ALWAYS present — pi always passes a config object —
//!   `{}` when nothing is set; carries `temperature`, `maxOutputTokens`,
//!   `thinkingConfig`), with `systemInstruction` (string →
//!   `{role: "user", parts: [{text}]}` via `tContent`), `tools`, and
//!   `toolConfig` hoisted to the top level. `samplingParams` never reaches
//!   the wire (upstream `buildParams` ignores it).
//! - Default SDK headers are `User-Agent`/`x-goog-api-client`
//!   (`google-genai-sdk/2.21.0 gl-node/<version>`) + `Content-Type`; pi's
//!   header record overrides `User-Agent` with the pi UA. The port sends the
//!   pi UA, caller headers, and `x-goog-api-key`; `x-goog-api-client` (pure
//!   Node-version telemetry) is omitted.
//!
//! Deviations from upstream, all structural (mirroring the sibling ports):
//! - `GoogleOptions` extension fields (`toolChoice`, `thinking`) have no port
//!   option surface on `StreamOptions`: direct `stream` calls pass none, and
//!   `streamSimple` derives them from the provider-neutral `toolChoice` and
//!   `reasoning`/`thinkingBudgets` — carried on the internal [`GoogleOptions`]
//!   so the pure builder and the wire stay testable.
//! - Ambient auth: gcloud CLI variant is a named error (bedrock/vertex env chains landed in M2d): the key resolves from
//!   `options.apiKey` then `ProviderConfig.api_key`, and a missing key is the
//!   async error event (upstream `streamSimple` throws synchronously; port
//!   contract). `options.signal` aborts and `fetch` injection have no port
//!   surface (M2a options omission); the two abort checks and the catch
//!   block's `"aborted"` branch are unreachable. `onPayload`
//!   (google-generative-ai.ts:96) rides the port's process-local
//!   `RequestCallbacks`: awaited before the send, `undefined` keeps the built
//!   params, any other value (including `null`) replaces them, and a hook
//!   rejection takes the catch path. Upstream has no onResponse call site —
//!   the response hook is accepted but never invoked.
//! - The stream chunk `finishReason` is kept as a raw string on the message
//!   (`rawStopReason`) and mapped by parsing it into the shared
//!   `GoogleFinishReason` enum and applying `map_stop_reason` (STOP → stop,
//!   MAX_TOKENS → length, every other enum value → error). Unknown provider
//!   strings throw `Unhandled stop reason: {raw}` mid-loop — upstream's
//!   `mapStopReason` default arm throws the same message, aborting the stream
//!   into the catch block — instead of failing chunk deserialization or
//!   silently continuing.
//! - SSE transport errors and malformed data payloads surface as the error
//!   event with the parse/transport message (upstream: the SDK's
//!   `SyntaxError` / `Incomplete JSON segment at the end` reach the same
//!   catch block). The SDK's in-stream `{error: {code: 4xx-5xx, status}}`
//!   chunk throws `got status: <status>. <chunk JSON>`; reproduced (JSON key
//!   order is serde-sorted, not JS insertion order).
//! - HTTP error bodies surface as the SDK's `ApiError` message:
//!   `JSON.stringify(<parsed body>)` for JSON bodies, the wrapped
//!   `{"error":{"message":<text>,"code":<status>,"status":<statusText>}}`
//!   otherwise (`throwErrorIfNotOK`). `formatProviderError` returns the
//!   message unchanged because the @google/genai error carries no separate
//!   body field. serde renders sorted keys; the status text is the canonical
//!   reason phrase (upstream reads `response.statusText`).
//! - `sanitizeSurrogates` is a no-op: Rust `String` is UTF-8 and cannot hold
//!   unpaired surrogates. The catch block's `index` property cleanup is a
//!   no-op: this adapter never sets one. Negative usage token counts saturate
//!   at 0 instead of going negative (u64 wire counts). A generated tool-call
//!   id renders a missing `functionCall.name` as `""` in the id prefix where
//!   the JS template literal would render `undefined`/`null`.
//! - URL resolution errors (`invalid model parameter`) happen before the
//!   retry loop, so `maxRetries` does not multiply the attempts for that one
//!   deterministic failure (upstream throws inside the retry callback).

#[cfg(test)]
mod callback_tests;

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::ai::api::google_shared::{
    convert_messages, convert_tools, get_disabled_google_thinking_config, map_stop_reason,
    resolve_google_function_calling_mode, resolve_google_thinking_level, retain_thought_signature,
    retry_google_request, supports_google_strict_tool_sampling, to_google_sdk_thinking_level,
    to_google_thinking_level, uses_google_thinking_level, GoogleApiThinkingLevel, GoogleContent,
    GoogleFinishReason, GoogleThinkingConfig, ResolvedGoogleThinkingLevel,
};
use crate::ai::api::openai_completions::request::{
    clamp_max_tokens_to_context, clamp_thinking_level, remove_header, set_header,
};
use crate::ai::api::{http_client, pi_user_agent, request_signal, ApiImpl, REQUEST_WAS_ABORTED};
use crate::ai::cost::calculate_cost;
use crate::ai::retry::ProviderError;
use crate::ai::transcript::{
    collapse_system_messages, get_current_tools, get_initial_system_message,
    get_system_message_text, TranscriptContext,
};
use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::message::{AssistantBlock, AssistantMessage};
use crate::ai::types::options::{SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{StopReason, ThinkingBudgets, ToolChoice, Usage};
use crate::ai::types::Model;
use crate::ai::{now_ms, ProviderConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The API id stamped on every emitted message.
const API: &str = "google-generative-ai";

/// SDK default Gemini API base + version (`GOOGLE_AI_API_DEFAULT_VERSION` =
/// `v1beta`); used when `model.baseUrl` is unset (pi then leaves
/// `httpOptions.baseUrl` at the SDK default).
const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Counter for generating unique tool call ids (upstream `toolCallCounter`,
/// rendered as `{name}_{Date.now()}_{++counter}`).
static TOOL_CALL_COUNTER: AtomicU32 = AtomicU32::new(0);

// =============================================================================
// Options (google-generative-ai.ts:47-54)
// =============================================================================

/// Upstream `GoogleOptions.thinking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GoogleThinkingOption {
    pub enabled: bool,
    /// `-1` requests dynamic thinking, `0` disables.
    pub budget_tokens: Option<i64>,
    pub level: Option<GoogleApiThinkingLevel>,
}

impl GoogleThinkingOption {
    fn disabled() -> Self {
        GoogleThinkingOption {
            enabled: false,
            budget_tokens: None,
            level: None,
        }
    }
}

/// Upstream `GoogleOptions` (extension fields over the base options):
/// internal because the public entry points default the extensions the way
/// the only reachable upstream flows do.
#[derive(Debug, Clone, Default)]
pub(crate) struct GoogleOptions {
    /// Base options (upstream `StreamOptions` inheritance).
    pub stream: StreamOptions,
    /// Upstream `toolChoice` (`"auto" | "none" | "any"`); `"any"` has no port
    /// surface (the provider-neutral union is `"auto" | "none"`).
    pub tool_choice: Option<String>,
    pub thinking: Option<GoogleThinkingOption>,
}

// =============================================================================
// Entry points
// =============================================================================

pub struct GoogleGenerativeAi;

impl ApiImpl for GoogleGenerativeAi {
    // Upstream wires `options?.onPayload?.(params, model)`
    // (google-generative-ai.ts:96); it has no onResponse call site, so only
    // the payload hook is supported here.
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
        // Upstream line 65: collapse system messages before the request task
        // (Gemini has no mid-conversation system messages).
        let collapsed = collapse_system_messages(ctx.clone());
        // Upstream direct-`stream` callers pass the API-specific extensions;
        // the port's `StreamOptions` surface carries none of them.
        run_stream(
            cfg.clone(),
            model.clone(),
            collapsed,
            options.clone(),
            Ok((None, None)),
        )
    }

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        // Upstream `streamSimple` (lines 304-345): buildBaseOptions shaping
        // (context-clamped maxTokens default over the ORIGINAL context), the
        // toolChoice passthrough, and the thinking resolution. A clamped
        // "off" and a missing reasoning both disable thinking. The extension
        // resolution may fail (unsupported thinking-level map); the error is
        // carried into the task AFTER the API-key check so the upstream throw
        // order (key first, map second) holds.
        let mut stream_options = options.stream.clone();
        stream_options.max_tokens = Some(clamp_max_tokens_to_context(
            model,
            ctx,
            options.stream.max_tokens.unwrap_or(model.max_tokens),
        ));
        let extensions = google_options_from_simple(model, options);
        run_stream(
            cfg.clone(),
            model.clone(),
            collapse_system_messages(ctx.clone()),
            stream_options,
            extensions,
        )
    }
}

/// The streamSimple-derived extension fields (upstream `GoogleOptions` minus
/// the base options): `toolChoice` and `thinking`, fallible because the
/// thinking-level map can reject. Kept separate from the base options so the
/// missing-key error can be resolved first inside the task.
type GoogleExtensions = Result<(Option<String>, Option<GoogleThinkingOption>), String>;

/// Upstream `streamSimple` thinking/toolChoice resolution (lines 314-344).
/// Falls with the `resolveGoogleThinkingLevel` error for unsupported maps.
fn google_options_from_simple(model: &Model, options: &SimpleStreamOptions) -> GoogleExtensions {
    // Upstream `toolChoice: options?.toolChoice` — the provider-neutral
    // `"auto" | "none"` forwards as the Google mode strings.
    let tool_choice = options
        .tool_choice
        .map(|choice| match choice {
            ToolChoice::Auto => "auto",
            ToolChoice::None => "none",
        })
        .map(str::to_string);

    let thinking = match options.reasoning {
        None => Some(GoogleThinkingOption::disabled()),
        Some(level) => match clamp_thinking_level(model, Some(level)) {
            // `clampThinkingLevel(model, ...) === "off"` disables thinking.
            None => Some(GoogleThinkingOption::disabled()),
            Some(clamped) => {
                let resolved = resolve_google_thinking_level(model, clamped)?;
                if uses_google_thinking_level(model) {
                    Some(GoogleThinkingOption {
                        enabled: true,
                        budget_tokens: None,
                        level: Some(to_google_thinking_level(resolved)),
                    })
                } else {
                    Some(GoogleThinkingOption {
                        enabled: true,
                        budget_tokens: Some(get_google_budget(
                            model,
                            resolved,
                            options.thinking_budgets.as_ref(),
                        )),
                        level: None,
                    })
                }
            }
        },
    };

    Ok((tool_choice, thinking))
}

/// Upstream `getGoogleBudget` (lines 430-470): custom budgets first, then the
/// per-family defaults matched on the RAW (not lowercased) model id, else
/// `-1` (dynamic thinking).
fn get_google_budget(
    model: &Model,
    level: ResolvedGoogleThinkingLevel,
    custom_budgets: Option<&ThinkingBudgets>,
) -> i64 {
    if let Some(budgets) = custom_budgets {
        let custom = match level {
            ResolvedGoogleThinkingLevel::Minimal => budgets.minimal,
            ResolvedGoogleThinkingLevel::Low => budgets.low,
            ResolvedGoogleThinkingLevel::Medium => budgets.medium,
            ResolvedGoogleThinkingLevel::High => budgets.high,
        };
        if let Some(budget) = custom {
            return i64::from(budget);
        }
    }

    let id = model.id.as_str();
    let budget_for = |minimal: i64, low: i64, medium: i64, high: i64| match level {
        ResolvedGoogleThinkingLevel::Minimal => minimal,
        ResolvedGoogleThinkingLevel::Low => low,
        ResolvedGoogleThinkingLevel::Medium => medium,
        ResolvedGoogleThinkingLevel::High => high,
    };
    if id.contains("2.5-pro") {
        return budget_for(128, 2048, 8192, 32768);
    }
    if id.contains("2.5-flash-lite") {
        return budget_for(512, 2048, 8192, 24576);
    }
    if id.contains("2.5-flash") {
        return budget_for(128, 2048, 8192, 24576);
    }
    -1
}

// =============================================================================
// Stream driver
// =============================================================================

fn run_stream(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    stream_options: StreamOptions,
    extensions: GoogleExtensions,
) -> mpsc::Receiver<AssistantMessageEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        run_stream_task(cfg, model, ctx, stream_options, extensions, tx).await;
    });
    rx
}

/// One open streamed block (upstream `currentBlock`): text or thinking, each
/// carrying the retained thought signature.
#[derive(Debug, Clone)]
enum OpenBlock {
    Text {
        text: String,
        text_signature: Option<String>,
    },
    Thinking {
        thinking: String,
        thinking_signature: Option<String>,
    },
}

impl OpenBlock {
    fn is_thinking(&self) -> bool {
        matches!(self, OpenBlock::Thinking { .. })
    }
}

async fn run_stream_task(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    stream_options: StreamOptions,
    extensions: GoogleExtensions,
    tx: mpsc::Sender<AssistantMessageEvent>,
) {
    let mut output = AssistantMessage {
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
    };
    let signal = request_signal(&stream_options.signal);
    let outcome: Result<(), String> = async {
        // Upstream line 87 (the fetch check has no port surface) and
        // streamSimple lines 309-312: the missing-key error throws FIRST
        // (the provider-config fallback is the port wiring, per the M2c
        // ruling), and the streamSimple thinking-map error (line 326) throws
        // second. Both surface as the lone error event (port contract).
        let api_key = resolve_api_key(&model, &cfg, &stream_options)?;
        let (tool_choice, thinking) = extensions?;
        let google = GoogleOptions {
            stream: stream_options,
            tool_choice,
            thinking,
        };
        let headers = build_headers(&model, &google);
        let params = build_params(&model, &ctx, &google)?;
        // Upstream lines 96-99: onPayload is awaited before the SDK request
        // (and the retry seam — a replacement applies to every attempt); JS
        // `undefined` preserves the built params while any other value —
        // including `null` — replaces them. A hook rejection throws into the
        // catch block before any HTTP traffic. The Google adapters have no
        // upstream onResponse surface (the callback is never invoked), so
        // only the payload hook is wired.
        let params = google
            .stream
            .callbacks
            .payload(params, &model)
            .await
            .map_err(|error| error.to_string())?;
        let url = resolve_endpoint(&model)?;

        let response = send_stream_request(&url, &api_key, &headers, &params, &google, &signal)
            .await
            .map_err(|error| error.message)?;

        // Upstream line 102: `start` after the response arrives.
        let _ = tx
            .send(AssistantMessageEvent::Start {
                message: output.clone(),
            })
            .await;

        let mut current_block: Option<OpenBlock> = None;
        let mut events = response.bytes_stream().eventsource();
        // Upstream `config.abortSignal` breaks the body reads on abort; the
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
            let payload: Value = serde_json::from_str(&event.data)
                .map_err(|error| format!("Could not parse Google SSE chunk: {error}"))?;
            // The SDK throws mid-stream when a data payload is a JSON error
            // object with a 4xx/5xx code (`got status: <status>. <chunk>`).
            if let Some(error_object) = payload.get("error") {
                let code = error_object.get("code").and_then(Value::as_i64);
                if code.is_some_and(|code| (400..600).contains(&code)) {
                    let status = match error_object.get("status") {
                        Some(Value::String(status)) => status.clone(),
                        Some(other) => other.to_string(),
                        None => "undefined".to_string(),
                    };
                    return Err(format!("got status: {status}. {payload}"));
                }
            }
            let chunk: GoogleStreamChunk = serde_json::from_value(payload)
                .map_err(|error| format!("Could not parse Google SSE chunk: {error}"))?;
            process_chunk(&mut output, &mut current_block, &chunk, &model, &tx).await?;
        }

        // Upstream lines 253-269: flush the open block.
        close_block(&mut output, current_block.take(), &tx).await;

        // Upstream line 271: the post-stream abort check precedes the
        // pending / error guards (lines 275-283).
        if signal.is_cancelled() {
            return Err(REQUEST_WAS_ABORTED.to_string());
        }
        if output.stop_reason == StopReason::Pending {
            return Err("Google stream ended without a finish reason".to_string());
        }
        if output.stop_reason == StopReason::Aborted || output.stop_reason == StopReason::Error {
            return Err(output
                .raw_stop_reason
                .as_deref()
                .map(|raw| format!("Provider stopped with: {raw}"))
                .unwrap_or_else(|| "An unknown error occurred".to_string()));
        }

        let reason = match output.stop_reason {
            StopReason::Length => SuccessReason::Length,
            StopReason::ToolUse => SuccessReason::ToolUse,
            _ => SuccessReason::Stop,
        };
        let _ = tx
            .send(AssistantMessageEvent::Done {
                reason,
                message: output.clone(),
            })
            .await;
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => {}
        Err(message) => {
            // Upstream catch block (lines 287-298): the `index` cleanup is a
            // no-op here (nothing sets one); stopReason settles to "aborted"
            // when the request signal fired, else "error", and the thrown
            // value becomes the errorMessage.
            let aborted = signal.is_cancelled();
            output.stop_reason = if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            output.error_message = Some(message);
            let _ = tx
                .send(AssistantMessageEvent::Error {
                    reason: if aborted {
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

/// Close one open block, emitting the authoritative `*_end` event at its
/// content index (upstream lines 120-136, 175-192, and 253-269 share this).
/// The accumulated text and retained signature are written back into the
/// message block — upstream mutates the block object in place (`currentBlock`
/// IS the stored block); the port's copies must be reconciled here.
async fn close_block(
    output: &mut AssistantMessage,
    block: Option<OpenBlock>,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    let Some(block) = block else {
        return;
    };
    let content_index = output.content.len() - 1;
    let event = match block {
        OpenBlock::Text {
            text,
            text_signature,
        } => {
            if let Some(AssistantBlock::Text(content)) = output.content.get_mut(content_index) {
                content.text = text.clone();
                content.text_signature = text_signature;
            }
            AssistantMessageEvent::TextEnd {
                content_index,
                content: text,
            }
        }
        OpenBlock::Thinking {
            thinking,
            thinking_signature,
        } => {
            if let Some(AssistantBlock::Thinking(content)) = output.content.get_mut(content_index) {
                content.thinking = thinking.clone();
                content.thinking_signature = thinking_signature;
            }
            AssistantMessageEvent::ThinkingEnd {
                content_index,
                content: thinking,
            }
        }
    };
    let _ = tx.send(event).await;
}

/// Process one `GenerateContentResponse` chunk (upstream loop body,
/// lines 106-251).
async fn process_chunk(
    output: &mut AssistantMessage,
    current_block: &mut Option<OpenBlock>,
    chunk: &GoogleStreamChunk,
    model: &Model,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    // Upstream lines 107-109: `responseId ||= chunk.responseId` — keep the
    // first non-empty one from the stream.
    if chunk
        .response_id
        .as_deref()
        .is_some_and(|response_id| !response_id.is_empty())
        && output
            .response_id
            .as_deref()
            .is_none_or(|existing| existing.is_empty())
    {
        output.response_id = chunk.response_id.clone();
    }

    if let Some(candidate) = chunk
        .candidates
        .as_ref()
        .and_then(|candidates| candidates.first())
    {
        if let Some(parts) = candidate
            .content
            .as_ref()
            .and_then(|content| content.parts.as_ref())
        {
            for part in parts {
                // Upstream gates text handling on `part.text !== undefined`
                // and function-call handling on `part.functionCall`
                // independently — a part can technically carry both.
                if let Some(text) = part.text.as_deref() {
                    process_text_part(output, current_block, part, text, tx).await;
                }
                if let Some(function_call) = part.function_call.as_ref() {
                    process_function_call_part(output, current_block, part, function_call, tx)
                        .await;
                }
            }
        }

        // Upstream lines 223-229: the raw string is recorded first
        // (`output.rawStopReason = candidate.finishReason`), then
        // `mapStopReason` maps the known enum values (STOP → stop,
        // MAX_TOKENS → length, every other enum value → error) and its
        // default arm THROWS `Unhandled stop reason: {raw}` for unknown
        // strings — aborting the loop into the catch block. A later valid
        // finish reason must not rescue the stream.
        if let Some(finish_reason) = candidate
            .finish_reason
            .as_deref()
            .filter(|finish_reason| !finish_reason.is_empty())
        {
            output.raw_stop_reason = Some(finish_reason.to_string());
            let reason = serde_json::from_value::<GoogleFinishReason>(Value::from(finish_reason))
                .map_err(|_| format!("Unhandled stop reason: {finish_reason}"))?;
            let mut stop_reason = map_stop_reason(reason);
            if stop_reason == StopReason::Stop
                && output
                    .content
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::ToolCall(_)))
            {
                stop_reason = StopReason::ToolUse;
            }
            output.stop_reason = stop_reason;
        }
    }

    // Upstream lines 231-250: usage metadata replaces the running usage and
    // the cost is recomputed from the model rates.
    if let Some(usage) = chunk.usage_metadata.as_ref() {
        let cached = usage.cached_content_token_count;
        output.usage = Usage {
            input: usage.prompt_token_count.saturating_sub(cached),
            output: usage
                .candidates_token_count
                .saturating_add(usage.thoughts_token_count),
            cache_read: cached,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: Some(usage.thoughts_token_count),
            total_tokens: usage.total_token_count,
            cost: Default::default(),
        };
        calculate_cost(model, &mut output.usage);
    }
    Ok(())
}

/// Upstream lines 112-172: text/thinking part handling with the block-kind
/// switch. The thinking test is the shared `isThinkingPart` rule:
/// `thought === true` is the definitive marker — `thoughtSignature` alone
/// never indicates thinking content.
async fn process_text_part(
    output: &mut AssistantMessage,
    current_block: &mut Option<OpenBlock>,
    part: &GoogleStreamPart,
    text: &str,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    let is_thinking = part.thought == Some(true);
    let keep_block = matches!(
        current_block.as_ref(),
        Some(block) if block.is_thinking() == is_thinking
    );
    if !keep_block {
        close_block(output, current_block.take(), tx).await;
        let content_index = output.content.len();
        if is_thinking {
            output
                .content
                .push(AssistantBlock::Thinking(ThinkingContent {
                    thinking: String::new(),
                    thinking_signature: None,
                    redacted: None,
                }));
            current_block.get_or_insert(OpenBlock::Thinking {
                thinking: String::new(),
                thinking_signature: None,
            });
            let _ = tx
                .send(AssistantMessageEvent::ThinkingStart { content_index })
                .await;
        } else {
            output.content.push(AssistantBlock::Text(TextContent {
                text: String::new(),
                text_signature: None,
            }));
            current_block.get_or_insert(OpenBlock::Text {
                text: String::new(),
                text_signature: None,
            });
            let _ = tx
                .send(AssistantMessageEvent::TextStart { content_index })
                .await;
        }
    }

    let content_index = output.content.len() - 1;
    match current_block.as_mut().expect("block just created") {
        OpenBlock::Thinking {
            thinking,
            thinking_signature,
        } => {
            thinking.push_str(text);
            *thinking_signature = retain_thought_signature(
                thinking_signature.as_deref(),
                part.thought_signature.as_deref(),
            );
            let _ = tx
                .send(AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta: text.to_string(),
                })
                .await;
        }
        OpenBlock::Text {
            text: block_text,
            text_signature,
        } => {
            block_text.push_str(text);
            *text_signature = retain_thought_signature(
                text_signature.as_deref(),
                part.thought_signature.as_deref(),
            );
            let _ = tx
                .send(AssistantMessageEvent::TextDelta {
                    content_index,
                    delta: text.to_string(),
                })
                .await;
        }
    }
}

/// Upstream lines 174-219: function-call part handling — close the open
/// block, generate an id when missing or duplicate, and emit the full
/// tool-call event triple.
async fn process_function_call_part(
    output: &mut AssistantMessage,
    current_block: &mut Option<OpenBlock>,
    part: &GoogleStreamPart,
    function_call: &GoogleStreamFunctionCall,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    if current_block.is_some() {
        close_block(output, current_block.take(), tx).await;
    }

    // Upstream lines 194-200: generate a unique id when none provided or the
    // provided one duplicates a tool call already in this message.
    let provided_id = function_call.id.as_deref().filter(|id| !id.is_empty());
    let needs_new_id = provided_id.is_none_or(|provided_id| {
        output
            .content
            .iter()
            .any(|block| matches!(block, AssistantBlock::ToolCall(call) if call.id == provided_id))
    });
    let tool_call_id = match provided_id {
        Some(id) if !needs_new_id => id.to_string(),
        _ => {
            // `${part.functionCall.name}_${Date.now()}_${++toolCallCounter}`
            // — the raw name renders like the JS template literal.
            let counter = TOOL_CALL_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
            format!(
                "{}_{}_{}",
                function_call.name.as_deref().unwrap_or("undefined"),
                now_ms(),
                counter
            )
        }
    };

    // `(part.functionCall.args as Record<string, any>) ?? {}` — missing and
    // null args both collapse to the empty object.
    let arguments = if function_call.args.is_null() {
        json!({})
    } else {
        function_call.args.clone()
    };
    let tool_call = ToolCall {
        id: tool_call_id,
        // `part.functionCall.name || ""`
        name: function_call.name.clone().unwrap_or_default(),
        arguments,
        // `...(part.thoughtSignature && { thoughtSignature })` — truthy only.
        thought_signature: part
            .thought_signature
            .as_deref()
            .filter(|signature| !signature.is_empty())
            .map(str::to_string),
        namespace: None,
    };
    output
        .content
        .push(AssistantBlock::ToolCall(tool_call.clone()));
    let content_index = output.content.len() - 1;
    let _ = tx
        .send(AssistantMessageEvent::ToolcallStart { content_index })
        .await;
    let _ = tx
        .send(AssistantMessageEvent::ToolcallDelta {
            content_index,
            delta: serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
        })
        .await;
    let _ = tx
        .send(AssistantMessageEvent::ToolcallEnd {
            content_index,
            tool_call,
        })
        .await;
}

// =============================================================================
// Request assembly (createClient, lines 347-366; buildParams, 368-428)
// =============================================================================

/// The request credential (upstream lines 90-93 read `options?.apiKey`): the
/// options key, then the provider credential `cfg.api_key` (the port's
/// wiring, per the M2c ruling). Upstream throws when both are missing —
/// before any thinking-map resolution error.
fn resolve_api_key(
    model: &Model,
    cfg: &ProviderConfig,
    stream_options: &StreamOptions,
) -> Result<String, String> {
    if let Some(key) = stream_options
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
    {
        return Ok(key.to_string());
    }
    if !cfg.api_key.is_empty() {
        return Ok(cfg.api_key.clone());
    }
    Err(format!("No API key for provider: {}", model.provider))
}

/// Upstream `tModel` (`@google/genai`): the URL path segment for the model —
/// `models/` prefixed unless the id already carries a recognized prefix;
/// `..`/`?`/`&` are rejected.
fn resolve_model_path(model_id: &str) -> Result<String, String> {
    if model_id.contains("..") || model_id.contains('?') || model_id.contains('&') {
        return Err("invalid model parameter".to_string());
    }
    Ok(
        if model_id.starts_with("models/") || model_id.starts_with("tunedModels/") {
            model_id.to_string()
        } else {
            format!("models/{model_id}")
        },
    )
}

/// The request URL (upstream `createClient` + `requestStream`):
/// `{baseUrl}/models/{model}:streamGenerateContent?alt=sse`. `model.baseUrl`
/// is the whole base (pi passes `apiVersion: ""`); the SDK default base +
/// `v1beta` otherwise. A trailing slash on the base is stripped
/// (`getRequestUrlInternal`).
fn resolve_endpoint(model: &Model) -> Result<String, String> {
    let base = if model.base_url.is_empty() {
        DEFAULT_BASE_URL.to_string()
    } else {
        model.base_url.trim_end_matches('/').to_string()
    };
    Ok(format!(
        "{base}/{}:streamGenerateContent?alt=sse",
        resolve_model_path(&model.id)?
    ))
}

/// Upstream `createClient` header assembly (line 357): the pi User-Agent,
/// model headers, and the caller's headers merged last (a `None` value —
/// upstream `null` — suppresses a default header).
fn build_headers(model: &Model, options: &GoogleOptions) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = vec![("User-Agent".to_string(), pi_user_agent())];
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
    headers
}

/// Upstream `buildParams` (lines 368-428) folded with the SDK's
/// `generateContentParametersToMldev` body shape.
fn build_params(
    model: &Model,
    ctx: &TranscriptContext,
    options: &GoogleOptions,
) -> Result<Value, String> {
    let contents: Vec<GoogleContent> = convert_messages(model, ctx);
    let initial_system_message = get_initial_system_message(ctx.messages());
    let current_tools = get_current_tools(ctx.messages());

    let supports_strict_mode = supports_google_strict_tool_sampling(&model.id);
    let function_calling_mode = if !current_tools.is_empty() {
        resolve_google_function_calling_mode(
            &current_tools,
            options.tool_choice.as_deref(),
            supports_strict_mode,
        )?
    } else {
        None
    };

    // Upstream line 390: empty when there is no initial system message (the
    // key is then omitted); `sanitizeSurrogates` is a no-op in Rust.
    let system_instruction = initial_system_message
        .map(get_system_message_text)
        .unwrap_or_default();

    let mut generation_config = Map::new();
    if let Some(temperature) = options.stream.temperature {
        generation_config.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = options.stream.max_tokens {
        generation_config.insert("maxOutputTokens".into(), json!(max_tokens));
    }
    // Upstream lines 402-412: level wins over budget; disabled thinking sends
    // the shared disabled config; non-reasoning models send nothing.
    if let Some(thinking) = &options.thinking {
        let thinking_config = if thinking.enabled && model.reasoning {
            let mut config = GoogleThinkingConfig {
                include_thoughts: Some(true),
                ..Default::default()
            };
            if let Some(level) = thinking.level {
                config.thinking_level = Some(to_google_sdk_thinking_level(level));
            } else if let Some(budget) = thinking.budget_tokens {
                config.thinking_budget = Some(budget);
            }
            Some(config)
        } else if model.reasoning {
            // `options.thinking` present but disabled.
            Some(get_disabled_google_thinking_config(model)?)
        } else {
            None
        };
        if let Some(thinking_config) = thinking_config {
            let wire = serde_json::to_value(thinking_config)
                .map_err(|error| format!("Could not serialize thinkingConfig: {error}"))?;
            generation_config.insert("thinkingConfig".into(), wire);
        }
    }

    let mut params = Map::new();
    let contents_wire = serde_json::to_value(contents)
        .map_err(|error| format!("Could not serialize contents: {error}"))?;
    params.insert("contents".into(), contents_wire);
    // The SDK always emits `generationConfig` when a config object is passed
    // (pi always passes one) — `{}` when nothing is set.
    params.insert("generationConfig".into(), Value::Object(generation_config));
    if !system_instruction.is_empty() {
        // The SDK converts the string via `tContent`:
        // `{role: "user", parts: [{text}]}`.
        params.insert(
            "systemInstruction".into(),
            json!({"role": "user", "parts": [{"text": system_instruction}]}),
        );
    }
    if !current_tools.is_empty() {
        if let Some(tools) = convert_tools(&current_tools, false, supports_strict_mode)? {
            params.insert("tools".into(), Value::Array(tools));
        }
    }
    if let Some(mode) = function_calling_mode {
        params.insert(
            "toolConfig".into(),
            json!({"functionCallingConfig": {"mode": mode}}),
        );
    }
    Ok(Value::Object(params))
}

/// Send the assembled request (upstream
/// `client.models.generateContentStream(params)`), wrapped in the shared
/// Google retry seam with `options.maxRetries`/`maxRetryDelayMs`. Retries
/// cover transport failures and retryable statuses only — once stream bytes
/// flow an error is never retried. The @google/genai SDK performs no internal
/// retries when constructed without `retryOptions` (pi passes none), so the
/// seam is the only retry layer. A cancelled signal fails the request with
/// the abort error.
async fn send_stream_request(
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    body: &Value,
    options: &GoogleOptions,
    signal: &CancellationToken,
) -> Result<reqwest::Response, ProviderError> {
    let mut header_map = reqwest::header::HeaderMap::new();
    let auth = reqwest::header::HeaderValue::from_str(api_key).map_err(|error| {
        ProviderError::transport(format!("Invalid x-goog-api-key header: {error}"))
    })?;
    header_map.insert("x-goog-api-key", auth);
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            ProviderError::transport(format!("Invalid header name \"{name}\": {error}"))
        })?;
        let value = reqwest::header::HeaderValue::from_str(value).map_err(|error| {
            ProviderError::transport(format!("Invalid header value for \"{name}\": {error}"))
        })?;
        header_map.insert(name, value);
    }
    let mut request = http_client().post(url).headers(header_map).json(body);
    if let Some(ms) = options.stream.timeout_ms {
        request = request.timeout(Duration::from_millis(ms));
    }
    let max_retries = options.stream.max_retries.unwrap_or(0);
    let max_retry_delay_ms = options.stream.max_retry_delay_ms;
    retry_google_request(max_retries, max_retry_delay_ms, Some(signal), || async {
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
        let content_type_is_json = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.contains("application/json"))
            .unwrap_or(false);
        let body_text = response.text().await.unwrap_or_default();
        Err(ProviderError::http(
            status_code,
            response_headers,
            format_google_http_error(status_code, content_type_is_json, &body_text),
        ))
    })
    .await
}

/// The @google/genai error body composition (`throwErrorIfNotOK`): JSON
/// bodies render as `JSON.stringify(<parsed body>)`; anything else wraps the
/// raw text in `{error: {message, code, status}}`. The pi catch block
/// surfaces the message unchanged (`formatProviderError` on an ApiError
/// without a body field returns `error.message`).
fn format_google_http_error(status: u16, content_type_is_json: bool, body_text: &str) -> String {
    let status_text = reqwest::StatusCode::from_u16(status)
        .ok()
        .as_ref()
        .and_then(reqwest::StatusCode::canonical_reason)
        .unwrap_or_default();
    if content_type_is_json {
        if let Ok(value) = serde_json::from_str::<Value>(body_text) {
            return value.to_string();
        }
    }
    json!({"error": {"message": body_text, "code": status, "status": status_text}}).to_string()
}

// =============================================================================
// Response wire types (`GenerateContentResponse` subset)
// =============================================================================

/// One SSE `data:` payload (`GenerateContentResponse` subset the port reads).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleStreamChunk {
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    candidates: Option<Vec<GoogleStreamCandidate>>,
    #[serde(default)]
    usage_metadata: Option<GoogleUsageMetadata>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleStreamCandidate {
    #[serde(default)]
    content: Option<GoogleStreamContent>,
    /// Kept raw: the string is preserved on the message (`rawStopReason`) and
    /// mapped with the shared string mapper, so unknown provider reasons
    /// survive like upstream's switch default instead of failing
    /// deserialization.
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleStreamContent {
    #[serde(default)]
    parts: Option<Vec<GoogleStreamPart>>,
}

/// Upstream `Part` subset the port consumes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleStreamPart {
    #[serde(default)]
    text: Option<String>,
    /// `isThinkingPart`: `thought === true` is the definitive thinking
    /// marker; `thoughtSignature` alone never indicates thinking content.
    #[serde(default)]
    thought: Option<bool>,
    #[serde(default)]
    thought_signature: Option<String>,
    #[serde(default)]
    function_call: Option<GoogleStreamFunctionCall>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleStreamFunctionCall {
    /// `part.functionCall.name || ""` upstream tolerates a missing name, so
    /// the field stays optional here.
    #[serde(default)]
    name: Option<String>,
    /// Missing and null args both collapse to `{}`.
    #[serde(default)]
    args: Value,
    #[serde(default)]
    id: Option<String>,
}

/// Upstream `usageMetadata`; every count is `|| 0` upstream, so missing
/// fields default to zero.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleUsageMetadata {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
    #[serde(default)]
    cached_content_token_count: u64,
    #[serde(default)]
    thoughts_token_count: u64,
    #[serde(default)]
    total_token_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::{abort_test_support::stalled_sse_server, REQUEST_ABORTED};
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::events::PartialAssistant;
    use crate::ai::types::message::{Message, StringOrBlocks, UserMessage};
    use crate::ai::types::options::ProviderHeaders;
    use crate::ai::types::primitives::{ModelCost, ThinkingLevelMap};
    use crate::ai::types::tool::{ConstrainedSampling, JsonSchemaSampling, Strict, Tool};
    use crate::ai::types::{ModelInput, ThinkingLevel};
    use serde_json::json;
    use std::sync::atomic::AtomicU32;

    const TS: i64 = 1758240000000;

    // ---- fixtures ----

    fn model(base_url: &str) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "gemini-2.5-flash".to_string(),
            name: "Gemini 2.5 Flash".to_string(),
            api: API.to_string(),
            provider: "google".to_string(),
            base_url: base_url.to_string(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: ModelCost {
                input: 0.3,
                output: 2.5,
                cache_read: 0.075,
                cache_write: 0.0,
                tiers: None,
            },
            context_window: 128000,
            max_tokens: 8192,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    fn model_with_id(base_url: &str, id: &str) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: id.to_string(),
            ..model(base_url)
        }
    }

    fn level_map(entries: &[(&str, Option<&str>)]) -> ThinkingLevelMap {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.map(str::to_string)))
            .collect()
    }

    fn ctx_with(messages: Vec<Message>) -> TranscriptContext {
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

    fn tool(name: &str) -> Tool {
        Tool {
            name: name.into(),
            description: "Echo a value".into(),
            parameters: json!({"type": "object", "properties": {"value": {"type": "string"}}}),
            constrained_sampling: None,
        }
    }

    fn strict_tool(name: &str, strict: Strict) -> Tool {
        Tool {
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict,
            })),
            ..tool(name)
        }
    }

    fn cfg() -> ProviderConfig {
        ProviderConfig {
            base_url: "https://unused.example.com".to_string(),
            api_key: "test-api-key".to_string(),
            max_tokens: 8192,
        }
    }

    fn sse(chunks: &[Value]) -> wiremock::ResponseTemplate {
        let body: String = chunks
            .iter()
            .map(|chunk| format!("data: {chunk}\n\n"))
            .collect();
        wiremock::ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body)
    }

    fn generate_content_path() -> String {
        "/v1beta/models/gemini-2.5-flash:streamGenerateContent".to_string()
    }

    async fn mount(server: &wiremock::MockServer, chunks: &[Value]) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(generate_content_path()))
            .respond_with(sse(chunks))
            .mount(server)
            .await;
    }

    async fn collect_stream(
        _server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = GoogleGenerativeAi;
        let mut rx = api.stream(&cfg(), model, ctx, options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    async fn collect_simple(
        _server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let api = GoogleGenerativeAi;
        let mut rx = api.stream_simple(&cfg(), model, ctx, options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    /// Runs one simple stream and returns the captured request (URL, headers,
    /// JSON body) and the event sequence.
    async fn capture_simple(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> (
        String,
        reqwest::header::HeaderMap,
        Value,
        Vec<AssistantMessageEvent>,
    ) {
        let events = collect_simple(server, model, ctx, options).await;
        let requests = server.received_requests().await.unwrap();
        assert!(!requests.is_empty(), "at least one request expected");
        let last = requests.last().unwrap();
        let body: Value = serde_json::from_slice(&last.body).unwrap();
        (last.url.to_string(), last.headers.clone(), body, events)
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

    fn error_of(events: &[AssistantMessageEvent]) -> AssistantMessage {
        match events.last() {
            Some(AssistantMessageEvent::Error { error, .. }) => error.clone(),
            other => panic!("expected terminal error, got {other:?}"),
        }
    }

    fn done_message(events: &[AssistantMessageEvent]) -> AssistantMessage {
        match events.last() {
            Some(AssistantMessageEvent::Done { message, .. }) => message.clone(),
            other => panic!("expected done, got {other:?}"),
        }
    }

    fn done_reason(events: &[AssistantMessageEvent]) -> SuccessReason {
        match events.last() {
            Some(AssistantMessageEvent::Done { reason, .. }) => *reason,
            other => panic!("expected done, got {other:?}"),
        }
    }

    fn body_of<'a>(header_map: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
        header_map.get(name).and_then(|value| value.to_str().ok())
    }

    fn text_chunk(text: &str) -> Value {
        json!({
            "candidates": [{"content": {"parts": [{"text": text}], "role": "model"}}]
        })
    }

    /// A bare STOP finish chunk (no content) — appends a normal end to a
    /// stream whose working chunks carry no finishReason.
    fn stop_chunk() -> Value {
        json!({"candidates": [{"finishReason": "STOP"}]})
    }

    fn usage_metadata_chunk() -> Value {
        json!({
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "cachedContentTokenCount": 3,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 17,
            }
        })
    }

    // ---- 1. wire shape: URL, auth, headers, body skeleton ----

    #[tokio::test]
    async fn wire_request_hits_stream_generate_content_with_alt_sse_and_api_key() {
        let server = wiremock::MockServer::start().await;
        mount(
            &server,
            &[text_chunk("hi"), stop_chunk(), usage_metadata_chunk()],
        )
        .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (url, headers, body, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;

        assert!(
            url.ends_with("models/gemini-2.5-flash:streamGenerateContent?alt=sse"),
            "{url}"
        );
        // Auth is the x-goog-api-key header; never a bearer token.
        assert_eq!(body_of(&headers, "x-goog-api-key"), Some("test-api-key"));
        assert_eq!(body_of(&headers, "authorization"), None);
        // The pi User-Agent rides by default.
        assert_eq!(
            body_of(&headers, "user-agent"),
            Some(pi_user_agent().as_str())
        );
        // Body: contents + always-present generationConfig; no tools/toolConfig.
        // streamSimple without a reasoning level disables thinking for the
        // budget-controlled model.
        assert_eq!(
            body["contents"],
            json!([{"role": "user", "parts": [{"text": "hello"}]}])
        );
        assert_eq!(
            body["generationConfig"],
            json!({
                "maxOutputTokens": 8192,
                "thinkingConfig": {"thinkingBudget": 0}
            })
        );
        assert!(body.get("tools").is_none(), "{body}");
        assert!(body.get("toolConfig").is_none(), "{body}");
        assert!(body.get("systemInstruction").is_none(), "{body}");
        assert!(body.get("model").is_none(), "model rides the URL: {body}");
        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "text_end", "done"],
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn explicit_headers_override_the_user_agent() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi")]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let mut option_headers = ProviderHeaders::new();
        option_headers.insert("User-Agent".to_string(), Some("custom-agent".to_string()));
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                headers: Some(option_headers),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, headers, _, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body_of(&headers, "user-agent"), Some("custom-agent"));
    }

    #[tokio::test]
    async fn model_base_url_is_used_whole_without_version_append() {
        let server = wiremock::MockServer::start().await;
        // A base URL that already carries the version path (pi passes
        // apiVersion: "" so no second version segment is appended).
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(
                "/custom/v1beta/models/gemini-2.5-flash:streamGenerateContent",
            ))
            .and(wiremock::matchers::query_param("alt", "sse"))
            .respond_with(sse(&[text_chunk("hi"), stop_chunk()]))
            .mount(&server)
            .await;
        let model = model(&format!("{}/custom/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (url, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert!(
            url.ends_with("/custom/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"),
            "{url}"
        );
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    #[test]
    fn default_endpoint_falls_back_to_the_sdk_base_and_version() {
        let model = model("");
        assert_eq!(
            resolve_endpoint(&model).unwrap(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn model_paths_keep_sdk_prefix_rules_and_reject_unsafe_ids() {
        assert_eq!(
            resolve_model_path("models/gemini-2.5-flash").unwrap(),
            "models/gemini-2.5-flash"
        );
        assert_eq!(
            resolve_model_path("tunedModels/my-tune").unwrap(),
            "tunedModels/my-tune"
        );
        assert_eq!(
            resolve_model_path("gemini-2.5-flash").unwrap(),
            "models/gemini-2.5-flash"
        );
        assert_eq!(
            resolve_model_path("bad?model").unwrap_err(),
            "invalid model parameter"
        );
        assert_eq!(
            resolve_model_path("../escape").unwrap_err(),
            "invalid model parameter"
        );
        assert_eq!(
            resolve_model_path("a&b").unwrap_err(),
            "invalid model parameter"
        );
    }

    #[tokio::test]
    async fn invalid_model_id_surfaces_the_sdk_error_message() {
        let server = wiremock::MockServer::start().await;
        let model = model_with_id(&format!("{}/v1beta", server.uri()), "bad?model");
        let ctx = ctx_with(vec![user_msg("hello")]);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("invalid model parameter")
        );
        assert_eq!(events.len(), 1, "lone error event: {events:?}");
        assert_eq!(error.api, API);
        assert_eq!(error.provider, "google");
        assert!(apply_all(&events).is_terminal());
    }

    #[tokio::test]
    async fn missing_api_key_is_a_lone_error_event() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi")]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let api = GoogleGenerativeAi;
        let mut request_cfg = cfg();
        request_cfg.api_key = String::new();
        let mut rx = api.stream_simple(&request_cfg, &model, &ctx, &SimpleStreamOptions::default());
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("No API key for provider: google")
        );
        assert_eq!(events.len(), 1);
        assert!(apply_all(&events).is_terminal());
    }

    #[tokio::test]
    async fn options_api_key_beats_provider_config() {
        let server = wiremock::MockServer::start().await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        mount(&server, &[text_chunk("hi"), stop_chunk()]).await;
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                api_key: Some("options-key".to_string()),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, headers, _, events) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body_of(&headers, "x-goog-api-key"), Some("options-key"));
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    // ---- 2. request body fields ----

    #[tokio::test]
    async fn temperature_and_max_tokens_fill_generation_config() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi"), stop_chunk()]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                temperature: Some(0.5),
                max_tokens: Some(128),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(
            body["generationConfig"],
            json!({"temperature": 0.5, "maxOutputTokens": 128, "thinkingConfig": {"thinkingBudget": 0}})
        );
    }

    #[tokio::test]
    async fn stream_simple_fills_context_clamped_default_max_tokens() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi"), stop_chunk()]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        // buildBaseOptions: clamp(maxTokens ?? model.maxTokens) — 8192 fits in
        // the 128000 context window.
        assert_eq!(body["generationConfig"]["maxOutputTokens"], json!(8192));
    }

    #[tokio::test]
    async fn direct_stream_sends_no_thinking_config_even_for_reasoning_models() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi"), stop_chunk()]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let events = collect_stream(&server, &model, &ctx, &StreamOptions::default()).await;
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert!(
            body["generationConfig"].get("thinkingConfig").is_none(),
            "{body}"
        );
    }

    #[tokio::test]
    async fn system_prompt_becomes_system_instruction() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi")]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = normalize_context(&Context {
            system_prompt: Some("You are precise.".to_string()),
            messages: vec![user_msg("hello")],
            tools: None,
        });
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(
            body["systemInstruction"],
            json!({"role": "user", "parts": [{"text": "You are precise."}]})
        );
        // The leading system message is not duplicated into contents.
        assert_eq!(
            body["contents"],
            json!([{"role": "user", "parts": [{"text": "hello"}]}])
        );
    }

    #[tokio::test]
    async fn tools_and_tool_choice_reach_the_wire() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi")]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("Use a tool.")],
            tools: Some(vec![tool("echo")]),
        });
        let options = SimpleStreamOptions {
            tool_choice: Some(ToolChoice::None),
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(
            body["tools"],
            json!([{"functionDeclarations": [{
                "name": "echo",
                "description": "Echo a value",
                "parametersJsonSchema": {
                    "type": "object",
                    "properties": {"value": {"type": "string"}},
                },
            }]}])
        );
        assert_eq!(
            body["toolConfig"],
            json!({"functionCallingConfig": {"mode": "NONE"}})
        );
    }

    #[tokio::test]
    async fn tool_choice_auto_maps_to_auto_mode() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi")]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("Use a tool.")],
            tools: Some(vec![tool("echo")]),
        });
        let options = SimpleStreamOptions {
            tool_choice: Some(ToolChoice::Auto),
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(
            body["toolConfig"],
            json!({"functionCallingConfig": {"mode": "AUTO"}})
        );
    }

    #[tokio::test]
    async fn strict_tool_on_gemini_3_forces_validated_mode() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi")]).await;
        let model = model_with_id(
            &format!("{}/v1beta", server.uri()),
            "gemini-3-flash-preview",
        );
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("Use a tool.")],
            tools: Some(vec![strict_tool("echo", Strict::Require)]),
        });
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(
            body["toolConfig"],
            json!({"functionCallingConfig": {"mode": "VALIDATED"}})
        );
    }

    #[tokio::test]
    async fn strict_tool_without_backend_support_is_a_lone_error_event() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi")]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("Use a tool.")],
            tools: Some(vec![strict_tool("echo", Strict::Require)]),
        });
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert!(error
            .error_message
            .as_deref()
            .unwrap_or_default()
            .starts_with("Tool \"echo\" requires JSON-schema constrained sampling"));
        assert_eq!(events.len(), 1);
    }

    // ---- 3. thinking payload capture (google-thinking-level-map /
    //         google-thinking-disable endpoint halves) ----

    async fn capture_thinking_config(
        server: &wiremock::MockServer,
        model: &Model,
        options: &SimpleStreamOptions,
    ) -> Value {
        let ctx = ctx_with(vec![user_msg("Hello")]);
        let (_, _, body, _) = capture_simple(server, model, &ctx, options).await;
        body["generationConfig"]["thinkingConfig"].clone()
    }

    #[tokio::test]
    async fn disabled_thinking_sends_budget_zero_for_gemini_2_5() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let thinking =
            capture_thinking_config(&server, &model, &SimpleStreamOptions::default()).await;
        assert_eq!(thinking, json!({"thinkingBudget": 0}));
    }

    #[tokio::test]
    async fn disabled_thinking_sends_budget_zero_for_gemini_3() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = model_with_id(
            &format!("{}/v1beta", server.uri()),
            "gemini-3-flash-preview",
        );
        let thinking =
            capture_thinking_config(&server, &model, &SimpleStreamOptions::default()).await;
        assert_eq!(thinking, json!({"thinkingBudget": 0}));
    }

    #[tokio::test]
    async fn omitted_reasoning_uses_the_lowest_supported_level() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        // Oracle (pi issue #9455): "uses the lowest supported level when
        // reasoning is omitted" — { off: null, minimal: null, low: "low" }.
        let mut model = model_with_id(&format!("{}/v1beta", server.uri()), "gemini-3.8-flash");
        model.thinking_level_map = Some(level_map(&[
            ("off", None),
            ("minimal", None),
            ("low", Some("low")),
            ("medium", Some("medium")),
            ("high", Some("high")),
            ("xhigh", None),
            ("max", None),
        ]));
        let thinking =
            capture_thinking_config(&server, &model, &SimpleStreamOptions::default()).await;
        assert_eq!(thinking, json!({"thinkingLevel": "LOW"}));
    }

    #[tokio::test]
    async fn reasoning_medium_keeps_native_level_for_gemini_3_1_pro() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = model_with_id(
            &format!("{}/v1beta", server.uri()),
            "gemini-3.1-pro-preview",
        );
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Medium),
            ..SimpleStreamOptions::default()
        };
        let thinking = capture_thinking_config(&server, &model, &options).await;
        assert_eq!(
            thinking,
            json!({"includeThoughts": true, "thinkingLevel": "MEDIUM"})
        );
    }

    #[tokio::test]
    async fn xhigh_and_max_map_to_a_supported_level() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            thinking_level_map: Some(level_map(&[("xhigh", Some("high")), ("max", Some("high"))])),
            ..model_with_id(&format!("{}/v1beta", server.uri()), "gemini-3.7-flash")
        };
        for reasoning in [ThinkingLevel::Xhigh, ThinkingLevel::Max] {
            let options = SimpleStreamOptions {
                reasoning: Some(reasoning),
                ..SimpleStreamOptions::default()
            };
            let thinking = capture_thinking_config(&server, &model, &options).await;
            assert_eq!(
                thinking,
                json!({"includeThoughts": true, "thinkingLevel": "HIGH"}),
                "reasoning: {reasoning:?}"
            );
        }
    }

    #[tokio::test]
    async fn uppercase_provider_values_are_honored() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            thinking_level_map: Some(level_map(&[("high", Some("LOW"))])),
            ..model_with_id(&format!("{}/v1beta", server.uri()), "gemini-3.7-flash")
        };
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..SimpleStreamOptions::default()
        };
        let thinking = capture_thinking_config(&server, &model, &options).await;
        assert_eq!(
            thinking,
            json!({"includeThoughts": true, "thinkingLevel": "LOW"})
        );
    }

    #[tokio::test]
    async fn custom_budgets_override_mapped_levels() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            thinking_level_map: Some(level_map(&[("xhigh", Some("high"))])),
            ..model_with_id(&format!("{}/v1beta", server.uri()), "gemini-2.5-flash")
        };
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Xhigh),
            thinking_budgets: Some(ThinkingBudgets {
                minimal: None,
                low: None,
                medium: None,
                high: Some(1234),
            }),
            ..SimpleStreamOptions::default()
        };
        let thinking = capture_thinking_config(&server, &model, &options).await;
        assert_eq!(
            thinking,
            json!({"includeThoughts": true, "thinkingBudget": 1234})
        );
    }

    #[tokio::test]
    async fn default_budgets_follow_the_model_family() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let base = format!("{}/v1beta", server.uri());
        let cases: [(&str, ThinkingLevel, i64); 6] = [
            ("gemini-2.5-pro", ThinkingLevel::High, 32768),
            ("gemini-2.5-pro", ThinkingLevel::Minimal, 128),
            ("gemini-2.5-flash", ThinkingLevel::High, 24576),
            ("gemini-2.5-flash-lite", ThinkingLevel::Medium, 8192),
            ("gemini-2.5-flash", ThinkingLevel::Low, 2048),
            // No family match → dynamic thinking.
            ("gemini-exp-1206", ThinkingLevel::Low, -1),
        ];
        for (id, reasoning, budget) in cases {
            let model = model_with_id(&base, id);
            let options = SimpleStreamOptions {
                reasoning: Some(reasoning),
                ..SimpleStreamOptions::default()
            };
            let thinking = capture_thinking_config(&server, &model, &options).await;
            assert_eq!(
                thinking,
                json!({"includeThoughts": true, "thinkingBudget": budget}),
                "{id} {reasoning:?}"
            );
        }
    }

    #[tokio::test]
    async fn unsupported_level_mapping_is_a_lone_error_event() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            thinking_level_map: Some(level_map(&[("xhigh", Some("extreme"))])),
            ..model_with_id(&format!("{}/v1beta", server.uri()), "gemini-2.5-flash")
        };
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Xhigh),
            ..SimpleStreamOptions::default()
        };
        let events =
            collect_simple(&server, &model, &ctx_with(vec![user_msg("hi")]), &options).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some(
                "Unsupported Google thinking level mapping for google/gemini-2.5-flash: xhigh -> extreme"
            )
        );
        assert_eq!(events.len(), 1);
    }

    // ---- 4. streaming block state machine ----

    #[tokio::test]
    async fn text_thinking_and_tool_call_blocks_stream_in_order() {
        let server = wiremock::MockServer::start().await;
        let chunks = [
            json!({"responseId": "resp-1", "candidates": [{"content": {"parts": [
                {"text": "pondering", "thought": true, "thoughtSignature": "sig-1"},
                {"text": "hello ", "thoughtSignature": "text-sig"},
            ]}}]}),
            json!({"responseId": "resp-2", "candidates": [{"content": {"parts": [
                {"text": "world"},
            ]}}]}),
            json!({"candidates": [{"content": {"parts": [
                {"functionCall": {"name": "echo", "args": {"value": "x"}}},
            ], "role": "model"}, "finishReason": "STOP"}]}),
            usage_metadata_chunk(),
        ];
        mount(&server, &chunks).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;

        assert_eq!(
            event_types(&events),
            [
                "start",
                "thinking_start",
                "thinking_delta",
                "thinking_end",
                "text_start",
                "text_delta",
                "text_delta",
                "text_end",
                "toolcall_start",
                "toolcall_delta",
                "toolcall_end",
                "done",
            ],
            "{events:?}"
        );

        let message = done_message(&events);
        assert_eq!(message.response_id.as_deref(), Some("resp-1"));
        // STOP is promoted to toolUse: the message carries a tool call.
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("STOP"));
        match message.content.as_slice() {
            [AssistantBlock::Thinking(thinking), AssistantBlock::Text(text), AssistantBlock::ToolCall(call)] =>
            {
                assert_eq!(thinking.thinking, "pondering");
                assert_eq!(thinking.thinking_signature.as_deref(), Some("sig-1"));
                assert_eq!(text.text, "hello world");
                assert_eq!(text.text_signature.as_deref(), Some("text-sig"));
                assert_eq!(call.name, "echo");
                assert_eq!(call.arguments, json!({"value": "x"}));
                // Generated id: `{name}_{Date.now()}_{counter}`.
                assert!(call.id.starts_with("echo_"), "id: {}", call.id);
                let rest = call.id.trim_start_matches("echo_");
                let parts: Vec<&str> = rest.split('_').collect();
                assert_eq!(parts.len(), 2, "id: {}", call.id);
                assert!(parts[0].parse::<i64>().is_ok());
                assert!(parts[1].parse::<u32>().is_ok());
                assert_eq!(call.thought_signature, None);
            }
            other => panic!("unexpected content: {other:?}"),
        }

        // Usage: input = prompt - cached, output = candidates + thoughts.
        assert_eq!(message.usage.input, 7);
        assert_eq!(message.usage.output, 7);
        assert_eq!(message.usage.cache_read, 3);
        assert_eq!(message.usage.reasoning, Some(2));
        assert_eq!(message.usage.total_tokens, 17);
        // Cost from the model rates (0.3/1M input, 2.5/1M output).
        assert!((message.usage.cost.input - 0.3 / 1_000_000.0 * 7.0).abs() < 1e-12);
        assert!((message.usage.cost.output - 2.5 / 1_000_000.0 * 7.0).abs() < 1e-12);

        // The event sequence replays into the same message through the
        // partial reducer.
        let partial = apply_all(&events);
        let replayed = partial.message().expect("replayed message");
        assert_eq!(replayed.content.len(), message.content.len());
    }

    #[tokio::test]
    async fn tool_call_ids_are_kept_and_duplicates_regenerated() {
        let server = wiremock::MockServer::start().await;
        let chunks = [json!({"candidates": [{"content": {"parts": [
            {"functionCall": {"id": "call-1", "name": "echo", "args": {}}},
            {"functionCall": {"id": "call-1", "name": "echo", "args": {}}},
            {"functionCall": {"name": "echo", "args": null}},
        ]}, "finishReason": "STOP"}]})];
        mount(&server, &chunks).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let message = done_message(&events);
        let calls: Vec<&ToolCall> = message
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].id, "call-1");
        assert_ne!(calls[1].id, "call-1", "duplicate id regenerated");
        assert!(calls[1].id.starts_with("echo_"));
        assert!(calls[2].id.starts_with("echo_"), "missing id generated");
        // Null args collapse to {}.
        assert_eq!(calls[2].arguments, json!({}));
    }

    #[tokio::test]
    async fn tool_call_carries_part_level_thought_signature() {
        let server = wiremock::MockServer::start().await;
        let chunks = [json!({"candidates": [{"content": {"parts": [
            {"functionCall": {"name": "echo", "args": {}}, "thoughtSignature": "call-sig"},
        ]}, "finishReason": "STOP"}]})];
        mount(&server, &chunks).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let message = done_message(&events);
        match message.content.first() {
            Some(AssistantBlock::ToolCall(call)) => {
                assert_eq!(call.thought_signature.as_deref(), Some("call-sig"));
            }
            other => panic!("unexpected content: {other:?}"),
        }
    }

    #[tokio::test]
    async fn first_non_empty_response_id_wins() {
        let server = wiremock::MockServer::start().await;
        let chunks = [
            json!({"responseId": "", "candidates": [{"content": {"parts": [{"text": "a"}]}}]}),
            json!({"responseId": "resp-real", "candidates": [{"content": {"parts": [{"text": "b"}]}}]}),
            stop_chunk(),
        ];
        mount(&server, &chunks).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(
            done_message(&events).response_id.as_deref(),
            Some("resp-real")
        );
    }

    #[tokio::test]
    async fn stream_without_finish_reason_is_an_error() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("partial")]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Google stream ended without a finish reason")
        );
        assert_eq!(error.stop_reason, StopReason::Error);
        // The partial text survives the error.
        assert_eq!(error.content.len(), 1);
    }

    #[tokio::test]
    async fn malformed_sse_payload_is_an_error_after_start() {
        let server = wiremock::MockServer::start().await;
        let good = json!({"candidates": [{"content": {"parts": [{"text": "ok"}]}}]});
        let body = format!("data: {{not json}}\n\ndata: {good}\n\n");
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(generate_content_path()))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(event_types(&events), ["start", "error"], "{events:?}");
        let error = error_of(&events);
        assert!(error
            .error_message
            .as_deref()
            .unwrap_or_default()
            .starts_with("Could not parse Google SSE chunk"));
    }

    // ---- 5. raw stop reasons (google-raw-stop-reason oracle) ----

    #[tokio::test]
    async fn malformed_function_call_preserves_raw_reason_as_error() {
        let server = wiremock::MockServer::start().await;
        mount(
            &server,
            &[
                json!({"candidates": [{"finishReason": "MALFORMED_FUNCTION_CALL"}]}),
                usage_metadata_chunk(),
            ],
        )
        .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(
            error.raw_stop_reason.as_deref(),
            Some("MALFORMED_FUNCTION_CALL")
        );
        assert_eq!(
            error.error_message.as_deref(),
            Some("Provider stopped with: MALFORMED_FUNCTION_CALL")
        );
    }

    #[tokio::test]
    async fn max_tokens_with_tool_call_maps_to_length() {
        let server = wiremock::MockServer::start().await;
        mount(
            &server,
            &[
                json!({"candidates": [{"content": {"parts": [
                    {"functionCall": {"id": "call-1", "name": "echo", "args": {"value": "truncated"}}},
                ]}, "finishReason": "MAX_TOKENS"}]}),
                usage_metadata_chunk(),
            ],
        )
        .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let message = done_message(&events);
        assert_eq!(message.stop_reason, StopReason::Length);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("MAX_TOKENS"));
        assert_eq!(done_reason(&events), SuccessReason::Length);
        assert!(message
            .content
            .iter()
            .any(|block| matches!(block, AssistantBlock::ToolCall(_))));
    }

    #[tokio::test]
    async fn stop_with_tool_call_maps_to_tool_use() {
        let server = wiremock::MockServer::start().await;
        mount(
            &server,
            &[
                json!({"candidates": [{"content": {"parts": [
                    {"functionCall": {"id": "call-1", "name": "echo", "args": {}}},
                ]}, "finishReason": "STOP"}]}),
                usage_metadata_chunk(),
            ],
        )
        .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let message = done_message(&events);
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("STOP"));
        assert_eq!(done_reason(&events), SuccessReason::ToolUse);
        assert!(message
            .content
            .iter()
            .any(|block| matches!(block, AssistantBlock::ToolCall(_))));
    }

    #[tokio::test]
    async fn unknown_finish_reasons_abort_the_stream_with_the_upstream_message() {
        let server = wiremock::MockServer::start().await;
        // A later valid finish reason must not rescue the stream: upstream's
        // mapStopReason throws on the unknown value and the loop never
        // resumes.
        mount(
            &server,
            &[
                json!({"candidates": [{"finishReason": "SOMETHING_NEW"}]}),
                json!({"candidates": [{"finishReason": "STOP"}]}),
            ],
        )
        .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(error.stop_reason, StopReason::Error);
        // The raw string is recorded before the throw (upstream line 224).
        assert_eq!(error.raw_stop_reason.as_deref(), Some("SOMETHING_NEW"));
        assert_eq!(
            error.error_message.as_deref(),
            Some("Unhandled stop reason: SOMETHING_NEW")
        );
    }

    /// Every known finish-reason wire string parses through the enum and maps
    /// like the shared `mapStopReason`; unknown strings do not parse — the
    /// mapping site turns that into the upstream `Unhandled stop reason` throw.
    #[test]
    fn every_known_finish_reason_string_parses_and_maps() {
        for (wire, expected) in [
            ("STOP", StopReason::Stop),
            ("MAX_TOKENS", StopReason::Length),
            ("FINISH_REASON_UNSPECIFIED", StopReason::Error),
            ("SAFETY", StopReason::Error),
            ("BLOCKLIST", StopReason::Error),
            ("PROHIBITED_CONTENT", StopReason::Error),
            ("SPII", StopReason::Error),
            ("RECITATION", StopReason::Error),
            ("LANGUAGE", StopReason::Error),
            ("OTHER", StopReason::Error),
            ("MALFORMED_FUNCTION_CALL", StopReason::Error),
            ("UNEXPECTED_TOOL_CALL", StopReason::Error),
            ("TOO_MANY_TOOL_CALLS", StopReason::Error),
            ("IMAGE_SAFETY", StopReason::Error),
            ("IMAGE_PROHIBITED_CONTENT", StopReason::Error),
            ("IMAGE_RECITATION", StopReason::Error),
            ("IMAGE_OTHER", StopReason::Error),
            ("NO_IMAGE", StopReason::Error),
        ] {
            let reason = serde_json::from_value::<GoogleFinishReason>(Value::from(wire))
                .unwrap_or_else(|error| panic!("{wire}: {error}"));
            assert_eq!(map_stop_reason(reason), expected, "{wire}");
        }
        assert!(
            serde_json::from_value::<GoogleFinishReason>(Value::from("SOMETHING_NEW")).is_err()
        );
    }

    #[tokio::test]
    async fn missing_key_wins_over_an_invalid_thinking_map() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            thinking_level_map: Some(level_map(&[("xhigh", Some("extreme"))])),
            ..model_with_id(&format!("{}/v1beta", server.uri()), "gemini-2.5-flash")
        };
        let ctx = ctx_with(vec![user_msg("hi")]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Xhigh),
            ..SimpleStreamOptions::default()
        };
        let api = GoogleGenerativeAi;
        let mut request_cfg = cfg();
        request_cfg.api_key = String::new();
        let mut rx = api.stream_simple(&request_cfg, &model, &ctx, &options);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        // Upstream streamSimple throws the missing-key error (lines 309-312)
        // before the thinking-map error (line 326).
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("No API key for provider: google")
        );
        assert_eq!(events.len(), 1);
        assert!(apply_all(&events).is_terminal());
    }

    // ---- 6. errors over the wire ----

    #[tokio::test]
    async fn http_error_surfaces_the_json_stringified_body() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(generate_content_path()))
            .respond_with(wiremock::ResponseTemplate::new(400).set_body_json(json!({
                "error": {"code": 400, "message": "bad request", "status": "INVALID_ARGUMENT"}
            })))
            .mount(&server)
            .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        // The @google/genai ApiError message retains the parsed body key order.
        assert_eq!(
            error.error_message.as_deref(),
            Some(r#"{"error":{"code":400,"message":"bad request","status":"INVALID_ARGUMENT"}}"#)
        );
        assert_eq!(events.len(), 1, "lone error event before start: {events:?}");
        assert!(apply_all(&events).is_terminal());
    }

    #[tokio::test]
    async fn non_json_error_body_wraps_in_the_error_object() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(generate_content_path()))
            .respond_with(
                wiremock::ResponseTemplate::new(401)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("bad key"),
            )
            .mount(&server)
            .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some(r#"{"error":{"message":"bad key","code":401,"status":"Unauthorized"}}"#)
        );
    }

    #[tokio::test]
    async fn retries_a_429_before_the_first_stream_byte() {
        let server = wiremock::MockServer::start().await;

        /// First response is an HTTP error, second (and later) succeed.
        struct Flaky {
            attempts: AtomicU32,
        }
        impl wiremock::Respond for Flaky {
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
                    sse(&[text_chunk("hi"), stop_chunk()])
                }
            }
        }

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(generate_content_path()))
            .respond_with(Flaky {
                attempts: AtomicU32::new(0),
            })
            .mount(&server)
            .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                max_retries: Some(1),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let events = collect_simple(&server, &model, &ctx, &options).await;
        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "text_end", "done"]
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn in_stream_error_chunk_surfaces_the_sdk_message() {
        let server = wiremock::MockServer::start().await;
        mount(
            &server,
            &[
                text_chunk("partial"),
                json!({"error": {"code": 429, "message": "Quota exceeded", "status": "RESOURCE_EXHAUSTED"}}),
            ],
        )
        .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some(
                r#"got status: RESOURCE_EXHAUSTED. {"error":{"code":429,"message":"Quota exceeded","status":"RESOURCE_EXHAUSTED"}}"#
            )
        );
        // The partial content survives.
        assert_eq!(error.content.len(), 1);
        assert_eq!(error.stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn in_stream_error_chunk_without_4xx_code_is_ignored() {
        let server = wiremock::MockServer::start().await;
        mount(
            &server,
            &[
                json!({"error": {"message": "no code here"}}),
                json!({"candidates": [{"finishReason": "STOP"}]}),
            ],
        )
        .await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(event_types(&events), ["start", "done"]);
    }

    #[tokio::test]
    async fn timeout_ms_is_applied_to_the_request() {
        // A timeout option must not break a healthy request.
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi"), stop_chunk()]).await;
        let model = model(&format!("{}/v1beta", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                timeout_ms: Some(10_000),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, _, _, events) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(
            event_types(&events),
            ["start", "text_start", "text_delta", "text_end", "done"]
        );
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
        let options = StreamOptions {
            signal: Some(token),
            ..StreamOptions::default()
        };
        let api = GoogleGenerativeAi;
        let mut rx = api.stream(
            &cfg(),
            &model(&server.uri()),
            &ctx_with(vec![user_msg("hi")]),
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
    /// stream aborted with `"Request was aborted"` (upstream line 271).
    #[tokio::test]
    async fn mid_stream_cancellation_settles_the_stream_aborted() {
        let base_url = stalled_sse_server().await;
        let token = CancellationToken::new();
        let options = StreamOptions {
            signal: Some(token.clone()),
            ..StreamOptions::default()
        };
        let api = GoogleGenerativeAi;
        let mut rx = api.stream(
            &cfg(),
            &model(&base_url),
            &ctx_with(vec![user_msg("hi")]),
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
}
