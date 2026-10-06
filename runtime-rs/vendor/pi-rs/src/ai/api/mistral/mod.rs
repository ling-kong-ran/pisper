//! Mistral conversations API — full port of the stream/streamSimple
//! implementations from upstream `packages/ai/src/api/mistral-conversations.ts`
//! (946 lines): the `{baseUrl}/v1/chat/completions` request, the native SSE
//! protocol (Mistral CompletionChunks, `data: [DONE]` terminator), the
//! streaming block state machine (text/thinking/toolCall keyed by wire
//! `tool_calls[].index`), the Mistral tool-call id normalizer
//! (`shortHash`-derived 9-char alphanumeric ids), the cached-token usage
//! accounting over every provider key shape, raw finish-reason preservation,
//! the prompt-cache `x-affinity`/`prompt_cache_key` pairing, and the
//! Mistral-specific reasoning controls (`prompt_mode: "reasoning"` for
//! Magistral, `reasoning_effort` for Mistral Small 4 / Medium / zai-glm-5-2).
//!
//! Deviations from upstream, all structural (mirroring the sibling ports):
//! - Upstream builds an SDK-style camelCase payload, lets `onPayload` mutate
//!   it, then remaps to the snake_case wire format (`toMistralWirePayload`).
//!   The port builds the wire payload directly, so the remap pass and the
//!   `onPayload`-only fields (`topP`, `randomSeed`,
//!   `responseFormat`/`json_schema`, `presencePenalty`, `frequencyPenalty`,
//!   `parallelToolCalls`, `safePrompt`) have no port input and are not ported.
//!   The oracle assertions against those fields are covered on the fields the
//!   port does set (max_tokens, prompt_mode, reasoning_effort, prompt_cache_key,
//!   tools, messages). The hooks themselves are wired (M2 correction):
//!   `onPayload` (line 147) is awaited before the request and observes the
//!   port's built payload — the only payload representation the port has,
//!   where upstream's hook saw the pre-remap camelCase shape — and
//!   `onResponse` (line 311) fires after the fetch resolves and before the
//!   ok check (non-success responses included).
//! - Ambient auth: gcloud CLI variant is a named error (bedrock/vertex env chains landed in M2d): the key resolves from
//!   `options.apiKey` then `ProviderConfig.api_key`, and a missing key is the
//!   async error event (upstream `streamSimple` throws synchronously; port
//!   contract). `options.signal` is the port's non-serialized
//!   [`CancellationToken`](tokio_util::sync::CancellationToken): a
//!   pre-cancelled token or cancellation during the initial request fails
//!   before `Start` with the retry seam's `"Request aborted"` abort error; a
//!   cancellation after `Start` breaks the body read (the
//!   `readMistralEvents` abort check) and the catch block's `"aborted"`
//!   branch settles the reason.
//! - Upstream applies `AbortSignal.timeout(options?.timeoutMs ?? 60_000)` — a
//!   60-second default even when unset. Per controller ruling the port behaves
//!   like its siblings: `timeout_ms` is applied only when provided (the shared
//!   client keeps a 60s connect timeout only). Transport timeouts surface as
//!   upstream's `AbortSignal.timeout` DOMException message ("The operation was
//!   aborted due to timeout") so the oracle's `/timeout/i` contract holds.
//! - Upstream is a plain `fetch` with no retry. Per controller ruling the
//!   initial request goes through the existing provider-request retry seam
//!   ([`crate::ai::retry::retry_provider_request`]) with `options.max_retries`
//!   (default 0 — no retry unless requested); once stream bytes flow an error
//!   is never retried.
//! - `onPayload` (mistral-conversations.ts:147) and `onResponse` (line 311)
//!   ride the port's process-local `RequestCallbacks`: onPayload is awaited
//!   before the request (`undefined` keeps the built payload, any other
//!   value — including `null` — replaces it), and onResponse fires after the
//!   fetch resolves and BEFORE the ok check — the hook observes non-success
//!   responses too, and a hook rejection fails the attempt ahead of the HTTP
//!   error path.
//! - SSE framing uses the shared spec-compliant parser
//!   (`eventsource_stream`): CR/LF/CRLF line terminators, blank-line event
//!   boundaries, multi-line `data:` joined with `\n`, exactly the shapes
//!   upstream's custom boundary scanner produced. Non-`data:` SSE fields are
//!   ignored by the spec parser where upstream's `data:`-line filter also
//!   ignored them. Malformed JSON surfaces the serde parse message where
//!   upstream surfaced the `JSON.parse` SyntaxError (same error-event path).
//! - `sanitizeSurrogates` is a no-op: Rust `String` is UTF-8 and cannot hold
//!   unpaired surrogates. The catch block's `partialArgs` scratch cleanup is
//!   structural: the scratch lives in stream state, and the error path parses
//!   the accumulated fragments into the tool-call blocks (upstream kept the
//!   live `parseStreamingJson` result on every delta) so partial arguments
//!   survive into the error message exactly like upstream.
//! - Negative usage token counts saturate at 0 instead of going negative
//!   (u64 wire counts); a `total_tokens` of 0 falls back to the sum like
//!   upstream's `||` chain.
//! - HTTP error bodies surface verbatim as
//!   `Mistral API error ({status}): {body}` (upstream `MistralHttpError` +
//!   `formatMistralError`); the no-body branch uses the canonical reason
//!   phrase (upstream reads `response.statusText`), falling back to
//!   `Request failed with status {code}` when the status has no reason phrase.
//! - Malformed-stream tolerances: a missing `toolCall.function.name` renders
//!   as `""` (upstream could carry `undefined`), and a non-array
//!   `content[].thinking` degrades to empty where upstream threw a TypeError —
//!   both only reachable with malformed provider payloads. `delta.content`
//!   scalars other than string/array error like upstream's non-iterable throw.
//!   Two more choices the serde layer makes on malformed payloads: a choice
//!   without `delta` deserializes to the default empty delta and is processed
//!   as a no-op (upstream's bare `choice.delta` access would TypeError into
//!   its catch block), and a `tool_calls[]` entry missing `function` fails the
//!   chunk's deserialization with serde's `missing field \`function\``
//!   message (upstream: the same TypeError-into-catch shape, different text).
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order — same documented deviation as the request-builder ports.

#[cfg(test)]
mod callback_tests;

use std::collections::HashMap;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::ai::api::openai_completions::request::{
    clamp_max_tokens_to_context, clamp_thinking_level, level_key, make_strict_json_schema,
    render_system_message_update, resolve_json_schema_strict_sampling, set_header, short_hash,
    transform_messages,
};
use crate::ai::api::openai_completions::stream::{parse_streaming_json, truncate_error_text};
use crate::ai::api::{http_client, pi_user_agent, request_signal, ApiImpl, REQUEST_WAS_ABORTED};
use crate::ai::cost::calculate_cost;
use crate::ai::retry::{retry_provider_request, ProviderError};
use crate::ai::transcript::{
    get_current_tools, get_system_message_text, resolve_transcript, TranscriptContext,
};
use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, TextOrImageBlock,
};
use crate::ai::types::options::{SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{CacheRetention, StopReason, ToolChoice, Usage};
use crate::ai::types::tool::Tool;
use crate::ai::types::{Model, ModelInput};
use crate::ai::{now_ms, ProviderConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The API id stamped on every emitted message.
const API: &str = "mistral-conversations";

/// Upstream `MISTRAL_TOOL_CALL_ID_LENGTH`: Mistral rejects tool-call ids
/// outside 9 alphanumeric characters.
const MISTRAL_TOOL_CALL_ID_LENGTH: usize = 9;

/// Upstream `MAX_MISTRAL_ERROR_BODY_CHARS`.
const MAX_MISTRAL_ERROR_BODY_CHARS: usize = 4000;

/// Placeholder for user images on text-only models (upstream
/// `transform-messages.ts` + the `toChatMessages` fallback — same string).
const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";

// =============================================================================
// Options (mistral-conversations.ts:34-40)
// =============================================================================

/// Upstream `MistralOptions` extension fields over the base options: internal
/// because the public entry points default the extensions the way the only
/// reachable upstream flows do (`streamSimple` derives them from the
/// provider-neutral `toolChoice`/`reasoning`; direct `stream` callers pass
/// none, matching upstream callers that omit them).
#[derive(Debug, Clone, Default)]
struct MistralOptions {
    /// Base options (upstream `StreamOptions` inheritance).
    stream: StreamOptions,
    /// Upstream `toolChoice`; the port surface produces `"auto"`/`"none"` via
    /// streamSimple (the other union arms have no port input).
    tool_choice: Option<String>,
    /// Upstream `promptMode: "reasoning"` (Magistral-style reasoning).
    prompt_mode: Option<String>,
    /// Upstream `reasoningEffort: "none" | "high"`.
    reasoning_effort: Option<String>,
}

// =============================================================================
// Entry points
// =============================================================================

pub struct MistralConversations;

impl ApiImpl for MistralConversations {
    // Upstream wires both `options?.onPayload?.(payload, model)`
    // (mistral-conversations.ts:147) and `options?.onResponse?.(...)`
    // (line 311, before the ok check).
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
        // Upstream line 130: resolveTranscript before the request task (Mistral
        // keeps mid-conversation system messages only when model.compat says
        // so). Direct-`stream` callers pass the API-specific extensions; the
        // port's `StreamOptions` surface carries none of them.
        let resolved = resolve_transcript(ctx.clone(), compat_supports_mid_convo(model));
        run_stream(
            cfg.clone(),
            model.clone(),
            resolved,
            MistralOptions {
                stream: options.clone(),
                ..MistralOptions::default()
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
        // Upstream `streamSimple` (lines 186-210): buildBaseOptions shaping
        // (context-clamped maxTokens default over the ORIGINAL context), the
        // toolChoice passthrough, and the reasoning-mode resolution. A clamped
        // "off" and a missing reasoning both disable the reasoning controls.
        let mut stream_options = options.stream.clone();
        stream_options.max_tokens = Some(clamp_max_tokens_to_context(
            model,
            ctx,
            options.stream.max_tokens.unwrap_or(model.max_tokens),
        ));
        let (tool_choice, prompt_mode, reasoning_effort) =
            mistral_options_from_simple(model, options);
        let resolved = resolve_transcript(ctx.clone(), compat_supports_mid_convo(model));
        run_stream(
            cfg.clone(),
            model.clone(),
            resolved,
            MistralOptions {
                stream: stream_options,
                tool_choice,
                prompt_mode,
                reasoning_effort,
            },
        )
    }
}

/// Upstream `streamSimple` reasoning/toolChoice resolution (lines 196-209):
/// `promptMode: "reasoning"` for reasoning models outside the effort family,
/// `reasoningEffort` (from `model.thinkingLevelMap[reasoning] ?? "high"`) for
/// the effort family, and the provider-neutral toolChoice passthrough.
fn mistral_options_from_simple(
    model: &Model,
    options: &SimpleStreamOptions,
) -> (Option<String>, Option<String>, Option<String>) {
    let tool_choice = options
        .tool_choice
        .map(|choice| match choice {
            ToolChoice::Auto => "auto",
            ToolChoice::None => "none",
        })
        .map(str::to_string);

    let reasoning = options
        .reasoning
        .and_then(|level| clamp_thinking_level(model, Some(level)));
    // Upstream (dc84c1ac0): models with a thinking level map use
    // `reasoning_effort` — the mapped level, or the map's `off` entry when
    // no level was requested; other reasoning models use `prompt_mode`.
    let effort_map = if model.reasoning {
        model.thinking_level_map.as_ref()
    } else {
        None
    };
    let reasoning_effort = effort_map.map(|map| match reasoning {
        Some(level) => mapped_reasoning_effort(map, level_key(Some(level))),
        None => map
            .get("off")
            .and_then(|entry| entry.clone())
            .unwrap_or_default(),
    });
    let prompt_mode = if model.reasoning && effort_map.is_none() && reasoning.is_some() {
        Some("reasoning".to_string())
    } else {
        None
    };
    (tool_choice, prompt_mode, reasoning_effort)
}

/// Upstream `effortMap[reasoning] ?? "high"`: the raw map value, `"high"`
/// when the level is absent or mapped to null.
fn mapped_reasoning_effort(
    map: &crate::ai::types::primitives::ThinkingLevelMap,
    key: &str,
) -> String {
    match map.get(key) {
        Some(Some(value)) => value.clone(),
        Some(None) | None => "high".to_string(),
    }
}

/// `model.compat?.supportsMidConvoSystemMessages` — the only compat flag the
/// Mistral adapter reads upstream.
fn compat_supports_mid_convo(model: &Model) -> Option<bool> {
    model
        .compat
        .as_ref()?
        .get("supportsMidConvoSystemMessages")?
        .as_bool()
}

// =============================================================================
// Stream driver
// =============================================================================

fn run_stream(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: MistralOptions,
) -> mpsc::Receiver<AssistantMessageEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        run_stream_task(cfg, model, ctx, options, tx).await;
    });
    rx
}

/// Which kind of streamed block is open (upstream `currentBlock.type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
}

/// The streaming accumulator (upstream `createOutput` + the loop locals):
/// `current_block` is `(content index, kind)` — the port mutates the stored
/// block in `output.content` directly, exactly like upstream's in-place
/// `currentBlock`, so partial text survives into error messages.
struct StreamState {
    output: AssistantMessage,
    current_block: Option<(usize, BlockKind)>,
    /// Streaming tool-call accumulators in first-appearance order (upstream
    /// `toolBlocksByKey`, a JS Map iterated in insertion order at finalize).
    tool_accs: Vec<ToolAcc>,
}

/// Streaming tool-call scratch (upstream the `partialArgs` field on the
/// toolCall block, keyed by wire index or derived id).
struct ToolAcc {
    key: ToolKey,
    content_index: usize,
    partial_args: String,
}

/// Upstream `toolBlocksByKey` key: `toolCall.index ?? callId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ToolKey {
    Index(u64),
    Id(String),
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
            current_block: None,
            tool_accs: Vec::new(),
        }
    }
}

async fn run_stream_task(
    cfg: ProviderConfig,
    model: Model,
    ctx: TranscriptContext,
    options: MistralOptions,
    tx: mpsc::Sender<AssistantMessageEvent>,
) {
    let mut state = StreamState::new(&model);
    let signal = request_signal(&options.stream.signal);
    let outcome: Result<(), String> = async {
        // Upstream lines 136-139 (inside the async body): the missing-key
        // error throws first — the provider-config fallback is the port
        // wiring per the M2c ruling.
        let api_key = resolve_api_key(&model, &cfg, &options.stream)?;
        // Upstream lines 141-144: cross-model tool-call id normalization with
        // the Mistral 9-char id normalizer.
        let normalizer = std::cell::RefCell::new(MistralToolCallIdNormalizer::default());
        let transformed = transform_messages(&model, ctx.messages(), &|id, _source| {
            normalizer.borrow_mut().normalize(id)
        });
        let payload = build_payload(&model, &ctx, &transformed, &options)?;
        // Upstream lines 147-150: onPayload is awaited before the request; JS
        // `undefined` preserves the built payload while any other value —
        // including `null` — replaces it. A hook rejection throws into the
        // catch block before any HTTP traffic.
        let payload = options
            .stream
            .callbacks
            .payload(payload, &model)
            .await
            .map_err(|error| error.to_string())?;
        let url = resolve_endpoint(&model.base_url)?;
        let headers = build_headers(&model, &api_key, &options.stream);

        let response =
            send_stream_request(&url, headers, &payload, &model, &options.stream, &signal).await?;

        // Upstream line 152: `start` after the response arrives.
        let _ = tx
            .send(AssistantMessageEvent::Start {
                message: state.output.clone(),
            })
            .await;

        consume_chat_stream(&mut state, response, &model, &signal, &tx).await?;

        // Upstream lines 155-164: the post-stream abort check precedes the
        // pending / error guards.
        if signal.is_cancelled() {
            return Err(REQUEST_WAS_ABORTED.to_string());
        }
        if state.output.stop_reason == StopReason::Pending {
            return Err("Mistral stream ended without a finish reason".to_string());
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
        Err(message) => {
            // Upstream catch block (lines 168-177): the partialArgs deletion
            // is the port's finalize-without-emitting (arguments keep the
            // parsed-so-far value like upstream's live parse); stopReason
            // settles to "aborted" when the request signal fired, else
            // "error", and the thrown value becomes the errorMessage.
            finalize_tool_blocks(&mut state, None).await;
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
                    error: state.output,
                })
                .await;
        }
    }
}

/// Upstream lines 136-139: `options?.apiKey` first (the provider-config
/// fallback is the port wiring), else the upstream missing-key error.
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

/// Close one open text/thinking block, emitting the authoritative `*_end`
/// event at its content index (upstream `finishCurrentBlock`). The block text
/// was accumulated in place in `output.content`.
async fn close_block(state: &mut StreamState, tx: &mpsc::Sender<AssistantMessageEvent>) {
    let Some((index, kind)) = state.current_block.take() else {
        return;
    };
    let event = match kind {
        BlockKind::Text => {
            let content = match state.output.content.get(index) {
                Some(AssistantBlock::Text(text)) => text.text.clone(),
                _ => String::new(),
            };
            AssistantMessageEvent::TextEnd {
                content_index: index,
                content,
            }
        }
        BlockKind::Thinking => {
            let content = match state.output.content.get(index) {
                Some(AssistantBlock::Thinking(thinking)) => thinking.thinking.clone(),
                _ => String::new(),
            };
            AssistantMessageEvent::ThinkingEnd {
                content_index: index,
                content,
            }
        }
    };
    let _ = tx.send(event).await;
}

/// Process one SSE loop iteration's payload (upstream loop body, lines
/// 589-732): response id capture, usage, finish reason, content items, and
/// fragmented tool calls.
async fn process_chunk(
    state: &mut StreamState,
    chunk: &MistralStreamChunk,
    model: &Model,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    // Upstream line 593: `output.responseId ||= chunk.id` — keep the first
    // non-empty one, mirroring OpenAI-style streaming ids.
    if chunk.id.as_deref().is_some_and(|id| !id.is_empty())
        && state
            .output
            .response_id
            .as_deref()
            .is_none_or(|existing| existing.is_empty())
    {
        state.output.response_id = chunk.id.clone();
    }

    // Upstream lines 595-607: usage replaces the running counts and the cost
    // is recomputed from the model rates.
    if let Some(usage) = &chunk.usage {
        let prompt_tokens = usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cached_prompt_tokens = cached_prompt_tokens(usage, prompt_tokens);
        state.output.usage.input = prompt_tokens.saturating_sub(cached_prompt_tokens);
        state.output.usage.output = usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        state.output.usage.cache_read = cached_prompt_tokens;
        state.output.usage.cache_write = 0;
        let total = usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        state.output.usage.total_tokens = if total > 0 {
            total
        } else {
            state.output.usage.input
                + state.output.usage.output
                + state.output.usage.cache_read
                + state.output.usage.cache_write
        };
        calculate_cost(model, &mut state.output.usage);
    }

    // Upstream line 609: only choices[0] is consumed.
    let Some(choice) = chunk.choices.first() else {
        return Ok(());
    };

    // Upstream lines 612-619: the raw finish reason is preserved and mapped;
    // a later chunk may overwrite it (last writer wins upstream too).
    if let Some(finish_reason) = choice
        .finish_reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
    {
        state.output.raw_stop_reason = Some(finish_reason.to_string());
        let (stop_reason, error_message) = map_chat_stop_reason(finish_reason);
        state.output.stop_reason = stop_reason;
        if let Some(error_message) = error_message {
            state.output.error_message = Some(error_message);
        }
    }

    // Upstream lines 621-683: delta content items.
    if let Some(content) = &choice.delta.content {
        process_content_items(state, content, tx).await?;
    }

    // Upstream lines 685-731: fragmented tool calls.
    for tool_call in choice.delta.tool_calls.as_deref().unwrap_or_default() {
        process_tool_call(state, tool_call, tx).await;
    }
    Ok(())
}

/// Upstream lines 623-682: the content-item loop. A string item appends to
/// (or opens) the text block; `{type: "thinking"}` items append to (or open)
/// the thinking block only when the joined delta is non-empty; `{type:
/// "text"}` items behave like string items; anything else is skipped.
async fn process_content_items(
    state: &mut StreamState,
    content: &Value,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let items: Vec<&Value> = match content {
        Value::String(_) => vec![content],
        Value::Array(items) => items.iter().collect(),
        // Upstream iterates `contentItems` — a scalar would throw
        // "is not iterable" into the catch block.
        other => {
            return Err(format!("Mistral delta content is not iterable: {other}"));
        }
    };
    for item in items {
        match item {
            Value::String(text) => {
                // Upstream (8930b9ec0): empty deltas never open a block.
                if text.is_empty() {
                    continue;
                }
                append_text_delta(state, text, tx).await;
            }
            Value::Object(object) => match object.get("type").and_then(Value::as_str) {
                Some("thinking") => {
                    // Upstream lines 644-647: parts joined, empty parts and
                    // empty results skipped before any block switch.
                    let delta = object
                        .get("thinking")
                        .and_then(Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|part| part.get("text").and_then(Value::as_str))
                                .filter(|text| !text.is_empty())
                                .collect::<Vec<&str>>()
                                .join("")
                        })
                        .unwrap_or_default();
                    if delta.is_empty() {
                        continue;
                    }
                    append_thinking_delta(state, &delta, tx).await;
                }
                Some("text") => {
                    let text = object
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    // Upstream (8930b9ec0): empty deltas never open a block.
                    if text.is_empty() {
                        continue;
                    }
                    append_text_delta(state, text, tx).await;
                }
                // Neither string nor thinking/text: skipped (upstream falls
                // through its branch chain).
                _ => {}
            },
            _ => {}
        }
    }
    Ok(())
}

/// Open-or-continue a text block and emit the delta (upstream lines
/// 627-640 / 666-681 share this).
async fn append_text_delta(
    state: &mut StreamState,
    text: &str,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    if !matches!(state.current_block, Some((_, BlockKind::Text))) {
        close_block(state, tx).await;
        let content_index = state.output.content.len();
        state.output.content.push(AssistantBlock::Text(TextContent {
            text: String::new(),
            text_signature: None,
        }));
        state.current_block = Some((content_index, BlockKind::Text));
        let _ = tx
            .send(AssistantMessageEvent::TextStart { content_index })
            .await;
    }
    if let Some(AssistantBlock::Text(block)) = state
        .output
        .content
        .get_mut(state.current_block.expect("block just opened").0)
    {
        block.text.push_str(text);
    }
    let content_index = state.current_block.expect("block just opened").0;
    let _ = tx
        .send(AssistantMessageEvent::TextDelta {
            content_index,
            delta: text.to_string(),
        })
        .await;
}

/// Open-or-continue a thinking block and emit the delta (upstream lines
/// 649-663).
async fn append_thinking_delta(
    state: &mut StreamState,
    text: &str,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    if !matches!(state.current_block, Some((_, BlockKind::Thinking))) {
        close_block(state, tx).await;
        let content_index = state.output.content.len();
        state
            .output
            .content
            .push(AssistantBlock::Thinking(ThinkingContent {
                thinking: String::new(),
                thinking_signature: None,
                redacted: None,
            }));
        state.current_block = Some((content_index, BlockKind::Thinking));
        let _ = tx
            .send(AssistantMessageEvent::ThinkingStart { content_index })
            .await;
    }
    if let Some(AssistantBlock::Thinking(block)) = state
        .output
        .content
        .get_mut(state.current_block.expect("block just opened").0)
    {
        block.thinking.push_str(text);
    }
    let content_index = state.current_block.expect("block just opened").0;
    let _ = tx
        .send(AssistantMessageEvent::ThinkingDelta {
            content_index,
            delta: text.to_string(),
        })
        .await;
}

/// Upstream lines 685-731: one streamed tool-call fragment — close any open
/// text/thinking block, resolve the call id (provider id when present and not
/// the literal `"null"`, else derived from `toolcall:{index}`), and accumulate
/// arguments by wire index (falling back to the id when the provider omits
/// the index).
async fn process_tool_call(
    state: &mut StreamState,
    tool_call: &MistralStreamToolCall,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) {
    if state.current_block.is_some() {
        close_block(state, tx).await;
    }
    let call_id = match tool_call.id.as_deref() {
        Some(id) if !id.is_empty() && id != "null" => id.to_string(),
        _ => derive_mistral_tool_call_id(&format!("toolcall:{}", tool_call.index.unwrap_or(0)), 0),
    };
    let key = match tool_call.index {
        Some(index) => ToolKey::Index(index),
        None => ToolKey::Id(call_id.clone()),
    };

    let content_index = if let Some(acc) = state.tool_accs.iter().find(|acc| acc.key == key) {
        acc.content_index
    } else {
        let call = ToolCall {
            id: call_id,
            name: tool_call.function.name.clone().unwrap_or_default(),
            arguments: json!({}),
            thought_signature: None,
            namespace: None,
        };
        state.output.content.push(AssistantBlock::ToolCall(call));
        let content_index = state.output.content.len() - 1;
        state.tool_accs.push(ToolAcc {
            key: key.clone(),
            content_index,
            partial_args: String::new(),
        });
        let _ = tx
            .send(AssistantMessageEvent::ToolcallStart { content_index })
            .await;
        content_index
    };

    // Upstream lines 719-722: string fragments append verbatim; object/absent
    // arguments serialize (`JSON.stringify(arguments || {})`).
    let args_delta = match &tool_call.function.arguments {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => "{}".to_string(),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
    };
    if let Some(acc) = state.tool_accs.iter_mut().find(|acc| acc.key == key) {
        acc.partial_args.push_str(&args_delta);
    }
    let _ = tx
        .send(AssistantMessageEvent::ToolcallDelta {
            content_index,
            delta: args_delta,
        })
        .await;
}

/// Upstream lines 734-749: finalize every tool-call block in first-appearance
/// order — re-parse the accumulated fragments, write the arguments into the
/// stored block, and (when emitting) send the authoritative `toolcall_end`.
/// The error path calls this with `tx: None` so partial arguments survive into
/// the error message like upstream's live `parseStreamingJson`.
async fn finalize_tool_blocks(
    state: &mut StreamState,
    tx: Option<&mpsc::Sender<AssistantMessageEvent>>,
) {
    let accs = std::mem::take(&mut state.tool_accs);
    for acc in accs {
        let arguments = parse_streaming_json(&acc.partial_args);
        let Some(block) = state.output.content.get_mut(acc.content_index) else {
            continue;
        };
        let AssistantBlock::ToolCall(call) = block else {
            continue;
        };
        call.arguments = arguments;
        if let Some(tx) = tx {
            let _ = tx
                .send(AssistantMessageEvent::ToolcallEnd {
                    content_index: acc.content_index,
                    tool_call: call.clone(),
                })
                .await;
        }
    }
}

/// The SSE consume loop (upstream `readMistralEvents` + `consumeChatStream`):
/// data lines are trimmed and joined by the shared spec parser; empty data
/// events are skipped, `data: [DONE]` ends the stream, and each payload must
/// be an object with a `choices` array (upstream `parseMistralEvent`).
async fn consume_chat_stream(
    state: &mut StreamState,
    response: reqwest::Response,
    model: &Model,
    signal: &CancellationToken,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let mut events = response.bytes_stream().eventsource();
    // Upstream `readMistralEvents` checks `signal.aborted` on every read; the
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
        let event = item.map_err(|error| match error {
            eventsource_stream::EventStreamError::Transport(error) => {
                format_transport_error(&error)
            }
            other => other.to_string(),
        })?;
        let data = event.data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            break;
        }
        let parsed: Value = serde_json::from_str(data).map_err(|error| error.to_string())?;
        if !parsed.is_object() || !parsed.get("choices").is_some_and(Value::is_array) {
            return Err("Invalid Mistral streaming event".to_string());
        }
        let chunk: MistralStreamChunk =
            serde_json::from_value(parsed).map_err(|error| error.to_string())?;
        process_chunk(state, &chunk, model, tx).await?;
    }

    // Upstream line 734: flush the open text/thinking block, then line 735:
    // finalize the tool calls.
    close_block(state, tx).await;
    finalize_tool_blocks(state, Some(tx)).await;
    Ok(())
}

// =============================================================================
// Tool-call id normalization (mistral-conversations.ts:232-262)
// =============================================================================

/// Upstream `createMistralToolCallIdNormalizer`: bijective map between the
/// transcript's tool-call ids and 9-char alphanumeric Mistral ids, so
/// cross-model replay survives the provider's id constraints.
#[derive(Debug, Default)]
struct MistralToolCallIdNormalizer {
    id_map: HashMap<String, String>,
    reverse_map: HashMap<String, String>,
}

impl MistralToolCallIdNormalizer {
    fn normalize(&mut self, id: &str) -> String {
        if let Some(existing) = self.id_map.get(id) {
            return existing.clone();
        }
        let mut attempt = 0u32;
        loop {
            let candidate = derive_mistral_tool_call_id(id, attempt);
            match self.reverse_map.get(&candidate) {
                Some(owner) if owner != id => {
                    attempt += 1;
                }
                _ => {
                    self.id_map.insert(id.to_string(), candidate.clone());
                    self.reverse_map.insert(candidate.clone(), id.to_string());
                    return candidate;
                }
            }
        }
    }
}

/// Upstream `deriveMistralToolCallId` (lines 254-262): a 9-char alphanumeric
/// id passes through at attempt 0; otherwise `shortHash` of the normalized id
/// (suffixed with the attempt), stripped and truncated to 9 chars.
fn derive_mistral_tool_call_id(id: &str, attempt: u32) -> String {
    let normalized: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if attempt == 0 && normalized.chars().count() == MISTRAL_TOOL_CALL_ID_LENGTH {
        return normalized;
    }
    let seed_base = if normalized.is_empty() {
        id.to_string()
    } else {
        normalized
    };
    let seed = if attempt == 0 {
        seed_base
    } else {
        format!("{seed_base}:{attempt}")
    };
    short_hash(&seed)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(MISTRAL_TOOL_CALL_ID_LENGTH)
        .collect()
}

// =============================================================================
// Request assembly (mistral-conversations.ts:292-530)
// =============================================================================

/// Upstream `buildChatPayload` (lines 508-530) folded with
/// `toMistralWirePayload` (the port emits wire keys directly — see the module
/// docs for the onPayload omission).
fn build_payload(
    model: &Model,
    ctx: &TranscriptContext,
    messages: &[Message],
    options: &MistralOptions,
) -> Result<Value, String> {
    let mut payload = Map::new();
    payload.insert("model".into(), json!(model.id));
    payload.insert("stream".into(), json!(true));
    payload.insert(
        "messages".into(),
        Value::Array(to_chat_messages(
            messages,
            model.input.contains(&ModelInput::Image),
        )),
    );

    let current_tools = get_current_tools(ctx.messages());
    if !current_tools.is_empty() {
        payload.insert(
            "tools".into(),
            Value::Array(to_function_tools(&current_tools)?),
        );
    }
    if let Some(temperature) = options.stream.temperature {
        payload.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = options.stream.max_tokens {
        payload.insert("max_tokens".into(), json!(max_tokens));
    }
    if let Some(tool_choice) = &options.tool_choice {
        payload.insert("tool_choice".into(), json!(tool_choice));
    }
    if let Some(prompt_mode) = &options.prompt_mode {
        payload.insert("prompt_mode".into(), json!(prompt_mode));
    }
    if let Some(reasoning_effort) = &options.reasoning_effort {
        payload.insert("reasoning_effort".into(), json!(reasoning_effort));
    }
    if should_use_prompt_caching(
        options.stream.cache_retention,
        options.stream.session_id.as_deref(),
    ) {
        payload.insert(
            "prompt_cache_key".into(),
            json!(options.stream.session_id.as_deref().unwrap_or_default()),
        );
    }
    Ok(Value::Object(payload))
}

/// Upstream `shouldUsePromptCaching` (lines 532-534): any retention but
/// `"none"`, plus a session id. Drives both the `prompt_cache_key` payload
/// field and the `x-affinity` header.
fn should_use_prompt_caching(retention: Option<CacheRetention>, session_id: Option<&str>) -> bool {
    retention != Some(CacheRetention::None) && session_id.is_some()
}

/// Upstream `toFunctionTools` (lines 752-765): strict sampling resolved with
/// full support (`resolveJsonSchemaStrictSampling(tool, true)`), the strict
/// schema subset when it resolves to true, and `strict: strict ?? false`.
/// `stripSymbolKeys` is a no-op here: `serde_json::Value` cannot hold symbol
/// keys.
fn to_function_tools(tools: &[Tool]) -> Result<Vec<Value>, String> {
    tools
        .iter()
        .map(|tool| {
            let strict = resolve_json_schema_strict_sampling(tool, true, None)?;
            let parameters = if strict == Some(true) {
                make_strict_json_schema(&tool.parameters, None)?
            } else {
                tool.parameters.clone()
            };
            Ok(json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": parameters,
                    "strict": strict.unwrap_or(false),
                },
            }))
        })
        .collect()
}

/// Upstream `toChatMessages` (lines 783-875): the wire message list. System
/// messages render in full at index 0 and as update renders afterwards;
/// user/assistant/tool content becomes text chunks, `image_url` data URLs,
/// thinking chunks, and `tool_calls` with stringified arguments.
fn to_chat_messages(messages: &[Message], supports_images: bool) -> Vec<Value> {
    let mut wire_messages: Vec<Value> = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        match message {
            Message::System(system) => {
                let text = if index == 0 {
                    get_system_message_text(system)
                } else {
                    render_system_message_update(system)
                };
                if !text.is_empty() {
                    wire_messages.push(json!({"role": "system", "content": text}));
                }
            }
            Message::User(user) => {
                match &user.content {
                    StringOrBlocks::Text(text) => {
                        wire_messages.push(json!({"role": "user", "content": text}));
                    }
                    StringOrBlocks::Blocks(blocks) => {
                        let had_images = blocks
                            .iter()
                            .any(|block| matches!(block, TextOrImageBlock::Image(_)));
                        let content: Vec<Value> = blocks
                        .iter()
                        .filter(|block| matches!(block, TextOrImageBlock::Text(_)) || supports_images)
                        .map(|block| match block {
                            TextOrImageBlock::Text(text) => json!({"type": "text", "text": text.text}),
                            TextOrImageBlock::Image(image) => json!({
                                "type": "image_url",
                                "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
                            }),
                        })
                        .collect();
                        if !content.is_empty() {
                            wire_messages.push(json!({"role": "user", "content": content}));
                        } else if had_images && !supports_images {
                            wire_messages.push(json!({"role": "user", "content": NON_VISION_USER_IMAGE_PLACEHOLDER}));
                        }
                    }
                }
            }
            Message::Assistant(assistant) => {
                let mut content_parts: Vec<Value> = Vec::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                for block in &assistant.content {
                    match block {
                        AssistantBlock::Text(text) => {
                            if !text.text.trim().is_empty() {
                                content_parts.push(json!({"type": "text", "text": text.text}));
                            }
                        }
                        AssistantBlock::Thinking(thinking) => {
                            if !thinking.thinking.trim().is_empty() {
                                content_parts.push(json!({
                                    "type": "thinking",
                                    "thinking": [{"type": "text", "text": thinking.thinking}],
                                }));
                            }
                        }
                        AssistantBlock::ToolCall(call) => {
                            let arguments = if call.arguments.is_null() {
                                json!({})
                            } else {
                                call.arguments.clone()
                            };
                            tool_calls.push(json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": serde_json::to_string(&arguments)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                },
                                "index": 0,
                            }));
                        }
                    }
                }
                if !content_parts.is_empty() || !tool_calls.is_empty() {
                    let mut wire = json!({"role": "assistant", "prefix": false});
                    let object = wire.as_object_mut().expect("literal object");
                    if !content_parts.is_empty() {
                        object.insert("content".into(), Value::Array(content_parts));
                    }
                    if !tool_calls.is_empty() {
                        object.insert("tool_calls".into(), Value::Array(tool_calls));
                    }
                    wire_messages.push(wire);
                }
            }
            Message::ToolResult(result) => {
                let text_result = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        TextOrImageBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<&str>>()
                    .join("\n");
                let has_images = result
                    .content
                    .iter()
                    .any(|block| matches!(block, TextOrImageBlock::Image(_)));
                let tool_text = build_tool_result_text(
                    &text_result,
                    has_images,
                    supports_images,
                    result.is_error,
                );
                let mut content = vec![json!({"type": "text", "text": tool_text})];
                if supports_images {
                    for block in &result.content {
                        if let TextOrImageBlock::Image(image) = block {
                            content.push(json!({
                                "type": "image_url",
                                "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
                            }));
                        }
                    }
                }
                wire_messages.push(json!({
                    "role": "tool",
                    "tool_call_id": result.tool_call_id,
                    "name": result.tool_name,
                    "content": content,
                }));
            }
        }
    }
    wire_messages
}

/// Upstream `buildToolResultText` (lines 877-896).
fn build_tool_result_text(
    text: &str,
    has_images: bool,
    supports_images: bool,
    is_error: bool,
) -> String {
    let trimmed = text.trim();
    let error_prefix = if is_error { "[tool error] " } else { "" };
    if !trimmed.is_empty() {
        let image_suffix = if has_images && !supports_images {
            "\n[tool image omitted: model does not support images]"
        } else {
            ""
        };
        return format!("{error_prefix}{trimmed}{image_suffix}");
    }
    if has_images {
        if supports_images {
            return if is_error {
                "[tool error] (see attached image)".to_string()
            } else {
                "(see attached image)".to_string()
            };
        }
        return if is_error {
            "[tool error] (image omitted: model does not support images)".to_string()
        } else {
            "(image omitted: model does not support images)".to_string()
        };
    }
    if is_error {
        "[tool error] (no tool output)".to_string()
    } else {
        "(no tool output)".to_string()
    }
}

// =============================================================================
// HTTP transport (mistral-conversations.ts:292-365)
// =============================================================================

/// Upstream `requestMistralStream` URL construction (lines 298-300): the base
/// URL's path loses trailing slashes, keeps one, and `v1/chat/completions`
/// joins onto it (WHATWG relative resolution, as `new URL`).
fn resolve_endpoint(base_url: &str) -> Result<String, String> {
    let mut base = reqwest::Url::parse(base_url).map_err(|_| format!("Invalid URL: {base_url}"))?;
    let path = base.path().trim_end_matches('/');
    base.set_path(&format!("{path}/"));
    base.join("v1/chat/completions")
        .map(|url| url.to_string())
        .map_err(|error| format!("Invalid URL: {error}"))
}

/// Upstream `buildMistralHeaders` (lines 336-353): the pi User-Agent, SSE
/// accept, bearer auth, and JSON content-type defaults; model headers then
/// caller headers merged on top with case-insensitive override semantics and
/// `null` (here `None`) deleting a default; `x-affinity` rides the session id
/// unless any override names that header.
fn build_headers(
    model: &Model,
    api_key: &str,
    stream_options: &StreamOptions,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = Vec::new();
    set_header(&mut headers, "User-Agent", &pi_user_agent());
    set_header(&mut headers, "accept", "text/event-stream");
    set_header(&mut headers, "authorization", &format!("Bearer {api_key}"));
    set_header(&mut headers, "content-type", "application/json");
    for (name, value) in model.headers.iter().flatten() {
        apply_header_override(&mut headers, name, value.as_deref());
    }
    if let Some(option_headers) = &stream_options.headers {
        for (name, value) in option_headers {
            apply_header_override(&mut headers, name, value.as_deref());
        }
    }
    let has_explicit_affinity = has_header_override(model.headers.as_ref(), "x-affinity")
        || has_header_override(stream_options.headers.as_ref(), "x-affinity");
    if should_use_prompt_caching(
        stream_options.cache_retention,
        stream_options.session_id.as_deref(),
    ) && !has_explicit_affinity
    {
        set_header(
            &mut headers,
            "x-affinity",
            stream_options.session_id.as_deref().unwrap_or_default(),
        );
    }
    headers
}

/// Case-insensitive set/delete (upstream `applyMistralHeaderOverrides` over a
/// `Headers` instance).
fn apply_header_override(headers: &mut Vec<(String, String)>, name: &str, value: Option<&str>) {
    match value {
        Some(value) => {
            match headers
                .iter_mut()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
            {
                Some(entry) => entry.1 = value.to_string(),
                None => headers.push((name.to_string(), value.to_string())),
            }
        }
        None => headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name)),
    }
}

/// Upstream `hasMistralHeaderOverride`: case-insensitive key presence, value
/// irrelevant (a `null` override still counts as explicit).
fn has_header_override(
    headers: Option<&crate::ai::types::options::ProviderHeaders>,
    target: &str,
) -> bool {
    headers
        .into_iter()
        .flatten()
        .any(|(name, _)| name.eq_ignore_ascii_case(target))
}

/// Send the assembled request (upstream `requestMistralStream`), wrapped in
/// the provider-request retry seam with `options.maxRetries` /
/// `maxRetryDelayMs` (upstream plain `fetch` performs no retries; the seam
/// defaults to 0). Retries cover transport failures and retryable statuses
/// only — once stream bytes flow an error is never retried.
async fn send_stream_request(
    url: &str,
    headers: Vec<(String, String)>,
    payload: &Value,
    model: &Model,
    stream_options: &StreamOptions,
    signal: &CancellationToken,
) -> Result<reqwest::Response, String> {
    let mut header_map = reqwest::header::HeaderMap::new();
    for (name, value) in &headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("Invalid header name \"{name}\": {error}"))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|error| format!("Invalid header value for \"{name}\": {error}"))?;
        header_map.insert(name, value);
    }
    let mut request = http_client().post(url).headers(header_map).json(payload);
    if let Some(ms) = stream_options.timeout_ms {
        request = request.timeout(Duration::from_millis(ms));
    }
    let max_retries = stream_options.max_retries.unwrap_or(0);
    let max_retry_delay_ms = stream_options.max_retry_delay_ms;
    retry_provider_request(max_retries, max_retry_delay_ms, Some(signal), || async {
        let response = request
            .try_clone()
            .expect("JSON request body is buffered and clonable")
            .send()
            .await
            .map_err(|error| ProviderError::transport(format_transport_error(&error)))?;
        // Upstream line 311: onResponse fires after the fetch resolves and
        // BEFORE the ok check — the hook observes non-success responses too,
        // and a hook rejection fails the attempt ahead of the HTTP error
        // path (upstream plain `fetch` performs no retries; with the port's
        // default `maxRetries: 0` the rejection surfaces immediately).
        stream_options
            .callbacks
            .http_response(&response, model)
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
            format_mistral_http_error(status_code, &body),
        ))
    })
    .await
    .map_err(|error| error.message)
}

/// Upstream `formatMistralError` over the `MistralHttpError` shape (lines
/// 264-281): a non-empty body rides the message verbatim (truncated at
/// `MAX_MISTRAL_ERROR_BODY_CHARS`), else the status text (upstream
/// `response.statusText`, `Request failed with status {code}` when unnamed).
fn format_mistral_http_error(status: u16, body_text: &str) -> String {
    let trimmed = body_text.trim();
    if !trimmed.is_empty() {
        return format!(
            "Mistral API error ({status}): {}",
            truncate_error_text(trimmed, MAX_MISTRAL_ERROR_BODY_CHARS)
        );
    }
    let status_text = reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|status| status.canonical_reason())
        .unwrap_or_default();
    if status_text.is_empty() {
        format!("Mistral API error ({status}): Request failed with status {status}")
    } else {
        format!("Mistral API error ({status}): {status_text}")
    }
}

/// Transport-failure messages. Upstream times out via
/// `AbortSignal.timeout`, whose DOMException message ("The operation was
/// aborted due to timeout") is what `formatMistralError` surfaces; reqwest's
/// own Display omits the timeout cause, so the upstream message is restored.
fn format_transport_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "The operation was aborted due to timeout".to_string();
    }
    error.to_string()
}

// =============================================================================
// Cached-token usage (mistral-conversations.ts:536-555)
// =============================================================================

/// Upstream `getMistralCachedPromptTokens`: the first non-null cached-token
/// value across every provider key shape, `0` for non-numeric values, clamped
/// into `[0, promptTokens]`.
fn cached_prompt_tokens(usage: &Value, prompt_tokens: u64) -> u64 {
    const CANDIDATES: [(&str, Option<&str>); 6] = [
        ("promptTokensDetails", Some("cachedTokens")),
        ("prompt_tokens_details", Some("cached_tokens")),
        ("promptTokenDetails", Some("cachedTokens")),
        ("prompt_token_details", Some("cached_tokens")),
        ("numCachedTokens", None),
        ("num_cached_tokens", None),
    ];
    let raw = CANDIDATES.iter().find_map(|(parent, child)| {
        let value = match child {
            Some(child) => usage.get(*parent).and_then(|entry| entry.get(*child)),
            None => usage.get(*parent),
        };
        match value {
            Some(value) if !value.is_null() => Some(value),
            _ => None,
        }
    });
    let cached = match raw.and_then(Value::as_f64) {
        Some(value) if value.is_finite() => value,
        _ => 0.0,
    };
    (prompt_tokens as f64).min(cached.max(0.0)).max(0.0) as u64
}

/// Upstream `mapChatStopReason` (lines 931-946): the raw reason is preserved
/// by the caller; the mapping pairs each known reason with the port stop
/// reason and, for provider-error stops, the surfaced error message.
fn map_chat_stop_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" => (StopReason::Stop, None),
        "length" | "model_length" => (StopReason::Length, None),
        "tool_calls" => (StopReason::ToolUse, None),
        "error" => (
            StopReason::Error,
            Some("Provider stopped with: error".to_string()),
        ),
        other => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
    }
}

// =============================================================================
// Response wire types (`MistralCompletionEvent` subset)
// =============================================================================

/// One SSE `data:` payload (the `MistralCompletionEvent.data` subset the port
/// reads).
#[derive(Debug, Clone, Default, Deserialize)]
struct MistralStreamChunk {
    #[serde(default)]
    id: Option<String>,
    /// Kept raw: the cached-token fallback chain reads six provider key
    /// shapes (`getMistralCachedPromptTokens`).
    #[serde(default)]
    usage: Option<Value>,
    #[serde(default)]
    choices: Vec<MistralStreamChoice>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MistralStreamChoice {
    /// Kept raw: the string is preserved on the message (`rawStopReason`)
    /// and mapped by [`map_chat_stop_reason`], so unknown provider reasons
    /// surface like upstream's switch default.
    #[serde(default, rename = "finish_reason")]
    finish_reason: Option<String>,
    #[serde(default)]
    delta: MistralStreamDelta,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MistralStreamDelta {
    /// String or content-chunk array; kept raw because the item loop branches
    /// per element type.
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    tool_calls: Option<Vec<MistralStreamToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
struct MistralStreamToolCall {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    index: Option<u64>,
    function: MistralStreamFunction,
}

#[derive(Debug, Clone, Deserialize)]
struct MistralStreamFunction {
    #[serde(default)]
    name: Option<String>,
    /// String fragments append verbatim; object arguments serialize
    /// (`JSON.stringify(arguments || {})`).
    #[serde(default)]
    arguments: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::{abort_test_support::stalled_sse_server, REQUEST_ABORTED};
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::content::{ImageContent, TextContent};
    use crate::ai::types::events::PartialAssistant;
    use crate::ai::types::message::{Message, StringOrBlocks, ToolResultMessage, UserMessage};
    use crate::ai::types::primitives::{CacheRetention, ModelCost, ThinkingLevelMap};
    use crate::ai::types::tool::{ConstrainedSampling, JsonSchemaSampling, Strict, Tool};
    use crate::ai::types::{ModelInput, ThinkingLevel};
    use serde_json::{json, Value};

    const TS: i64 = 1758240000000;

    // ---- fixtures ----

    fn model(base_url: &str) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "mistral-large-latest".to_string(),
            name: "Mistral Large".to_string(),
            api: API.to_string(),
            provider: "mistral".to_string(),
            base_url: base_url.to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: ModelCost {
                input: 2.0,
                output: 6.0,
                cache_read: 0.0,
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

    fn model_with_id(base_url: &str, id: &str, reasoning: bool) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: id.to_string(),
            reasoning,
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

    fn image(data: &str) -> ImageContent {
        ImageContent {
            data: data.to_string(),
            mime_type: "image/png".to_string(),
        }
    }

    fn tool(name: &str) -> Tool {
        Tool {
            name: name.into(),
            description: "Look something up".into(),
            parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}}),
            constrained_sampling: None,
        }
    }

    fn strict_tool(name: &str, strict: Strict) -> Tool {
        Tool {
            parameters: json!({
                "type": "object",
                "properties": {"nested": {"type": "object", "properties": {"value": {"type": "string"}}}},
            }),
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

    /// A terminal Mistral CompletionChunk with the given finish reason.
    fn terminal_event(finish_reason: &str) -> Value {
        json!({
            "id": "mistral-response-id",
            "model": "mistral-large-latest",
            "choices": [{"index": 0, "finish_reason": finish_reason, "delta": {}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        })
    }

    fn sse(events: &[Value]) -> wiremock::ResponseTemplate {
        let mut body = String::new();
        for event in events {
            body.push_str(&format!("data: {event}\r\n\r\n"));
        }
        body.push_str("data: [DONE]\r\n\r\n");
        wiremock::ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body)
    }

    async fn mount(server: &wiremock::MockServer, events: &[Value]) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(sse(events))
            .mount(server)
            .await;
    }

    async fn collect_stream(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> Vec<AssistantMessageEvent> {
        let _ = server;
        let api = MistralConversations;
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
        let api = MistralConversations;
        let mut rx = api.stream_simple(&cfg(), model, ctx, options);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    /// Runs one simple stream and returns (request URL, headers, JSON body,
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

    #[allow(dead_code)]
    fn done_message(events: &[AssistantMessageEvent]) -> AssistantMessage {
        match events.last() {
            Some(AssistantMessageEvent::Done { message, .. }) => message.clone(),
            other => panic!("expected done, got {other:?}"),
        }
    }

    fn body_of<'a>(header_map: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
        header_map.get(name).and_then(|value| value.to_str().ok())
    }
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
        let request = requests.last().unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        (
            request.url.to_string(),
            request.headers.clone(),
            body,
            events,
        )
    }

    async fn capture_stream(
        server: &wiremock::MockServer,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> (
        String,
        reqwest::header::HeaderMap,
        Value,
        Vec<AssistantMessageEvent>,
    ) {
        let events = collect_stream(server, model, ctx, options).await;
        let requests = server.received_requests().await.unwrap();
        assert!(!requests.is_empty(), "at least one request expected");
        let request = requests.last().unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        (
            request.url.to_string(),
            request.headers.clone(),
            body,
            events,
        )
    }
    /// Upstream `mistral-reasoning-mode.test.ts` (the dc84c1ac0 delta):
    /// models WITH a thinking level map use `reasoning_effort` — the mapped
    /// level (clamped unsupported levels fall through to `"high"`), the
    /// map's `"off"` entry when no level is requested — while reasoning
    /// models WITHOUT a map (Magistral) use `prompt_mode`; non-reasoning
    /// models omit both.
    #[tokio::test]
    async fn reasoning_mode_selection_follows_the_thinking_level_map_presence() {
        let server = wiremock::MockServer::start().await;
        let ctx = ctx_with(vec![Message::User(UserMessage {
            content: StringOrBlocks::Text("Hello".to_string()),
            timestamp: TS,
        })]);

        async fn controls_of(
            server: &wiremock::MockServer,
            ctx: &TranscriptContext,
            id: &str,
            reasoning: bool,
            map: Option<ThinkingLevelMap>,
            options: &SimpleStreamOptions,
        ) -> (Option<Value>, Option<Value>) {
            mount(server, &[terminal_event("stop")]).await;
            let mut model = model_with_id(&server.uri(), id, reasoning);
            model.thinking_level_map = map;
            let (_, _, body, events) = capture_simple(server, &model, ctx, options).await;
            assert!(
                matches!(events.last(), Some(AssistantMessageEvent::Done { .. })),
                "{id}: {events:?}"
            );
            (
                body.get("prompt_mode").cloned(),
                body.get("reasoning_effort").cloned(),
            )
        }

        let reasoning_high = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..SimpleStreamOptions::default()
        };
        let reasoning_off = SimpleStreamOptions::default();
        let none_high = level_map(&[("high", Some("high")), ("off", Some("none"))]);

        // Reasoning models without a map (Magistral) use prompt_mode.
        let (prompt_mode, effort) = controls_of(
            &server,
            &ctx,
            "magistral-medium-latest",
            true,
            None,
            &reasoning_high,
        )
        .await;
        assert_eq!(prompt_mode, Some(json!("reasoning")));
        assert_eq!(effort, None);

        // ...and omit both when thinking is off.
        let (prompt_mode, effort) = controls_of(
            &server,
            &ctx,
            "magistral-medium-latest",
            true,
            None,
            &reasoning_off,
        )
        .await;
        assert_eq!(prompt_mode, None);
        assert_eq!(effort, None);

        // Mapped models (#8700/#9375): reasoning_effort from the map, no
        // prompt_mode; thinking off sends the map's "off" entry ("none").
        for id in ["mistral-small-2603", "mistral-medium-latest", "zai-glm-5-2"] {
            let (prompt_mode, effort) = controls_of(
                &server,
                &ctx,
                id,
                true,
                Some(none_high.clone()),
                &reasoning_high,
            )
            .await;
            assert_eq!(prompt_mode, None, "{id}");
            assert_eq!(effort, Some(json!("high")), "{id}");
            let (prompt_mode, effort) = controls_of(
                &server,
                &ctx,
                id,
                true,
                Some(none_high.clone()),
                &reasoning_off,
            )
            .await;
            assert_eq!(prompt_mode, None, "{id}");
            assert_eq!(effort, Some(json!("none")), "{id}");
        }

        // Non-reasoning models omit both even with a map and a level.
        let (prompt_mode, effort) = controls_of(
            &server,
            &ctx,
            "mistral-medium-2505",
            false,
            Some(none_high),
            &reasoning_high,
        )
        .await;
        assert_eq!(prompt_mode, None);
        assert_eq!(effort, None);
    }

    #[tokio::test]
    async fn reasoning_effort_follows_the_thinking_level_map() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[terminal_event("stop")]).await;
        let ctx = ctx_with(vec![user_msg("hello")]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Medium),
            ..SimpleStreamOptions::default()
        };
        // A mapped value wins over the "high" default, including "none".
        let mut model = model_with_id(&server.uri(), "mistral-small-2603", true);
        model.thinking_level_map = Some(level_map(&[("medium", Some("none"))]));
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["reasoning_effort"], json!("none"));

        // A null mapping falls through to "high" (upstream `?? "high"`).
        let mut model = model_with_id(&server.uri(), "mistral-small-2603", true);
        model.thinking_level_map = Some(level_map(&[("medium", None)]));
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    /// The provider-neutral toolChoice passthrough reaches the wire as the
    /// mistral `tool_choice` string (upstream lines 196-209 + 966-968); the
    /// key is omitted when no choice is requested.
    #[tokio::test]
    async fn tool_choice_reaches_the_wire() {
        let server = wiremock::MockServer::start().await;
        let ctx = ctx_with(vec![user_msg("hello")]);
        let model = model(&server.uri());

        mount(&server, &[terminal_event("stop")]).await;
        let options = SimpleStreamOptions {
            tool_choice: Some(ToolChoice::None),
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["tool_choice"], json!("none"));

        mount(&server, &[terminal_event("stop")]).await;
        let options = SimpleStreamOptions {
            tool_choice: Some(ToolChoice::Auto),
            ..SimpleStreamOptions::default()
        };
        let (_, _, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["tool_choice"], json!("auto"));

        mount(&server, &[terminal_event("stop")]).await;
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert!(body.get("tool_choice").is_none(), "{body}");
    }

    #[tokio::test]
    async fn prompt_cache_key_follows_cache_retention() {
        let server = wiremock::MockServer::start().await;
        let ctx = ctx_with(vec![user_msg("hello")]);
        let model = model(&server.uri());

        mount(&server, &[terminal_event("stop")]).await;
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                session_id: Some("session-123".to_string()),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, request_headers, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body["prompt_cache_key"], json!("session-123"));
        assert_eq!(body_of(&request_headers, "x-affinity"), Some("session-123"));

        mount(&server, &[terminal_event("stop")]).await;
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                session_id: Some("session-123".to_string()),
                cache_retention: Some(CacheRetention::None),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, request_headers, body, _) = capture_simple(&server, &model, &ctx, &options).await;
        assert!(body.get("prompt_cache_key").is_none(), "{body}");
        assert_eq!(request_headers.get("x-affinity"), None);
    }

    // ---- 10. tool schema serialization (mistral-tool-schema oracle) ----

    #[tokio::test]
    async fn strict_tool_schema_is_converted_on_the_wire() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[terminal_event("stop")]).await;
        let model = model_with_id(&server.uri(), "devstral-medium-latest", false);
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("Hi")],
            tools: Some(vec![strict_tool("inspect_schema", Strict::Require)]),
        });
        let (_, _, body, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;

        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
        let function = &body["tools"][0]["function"];
        assert_eq!(function["strict"], json!(true));
        assert_eq!(function["name"], json!("inspect_schema"));
        // The strict-converted schema (upstream makeStrictJsonSchema): every
        // object level gets required + additionalProperties:false, and
        // non-required properties gain a nullable anyOf arm.
        assert_eq!(
            function["parameters"],
            json!({
                "type": "object",
                "properties": {
                    "nested": {
                        "anyOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "value": {
                                        "anyOf": [{"type": "string"}, {"type": "null"}],
                                    },
                                },
                                "required": ["value"],
                                "additionalProperties": false,
                            },
                            {"type": "null"},
                        ],
                    },
                },
                "required": ["nested"],
                "additionalProperties": false,
            }),
            "{body}"
        );
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    #[tokio::test]
    async fn plain_tools_fall_back_to_verbatim_parameters() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[terminal_event("stop")]).await;
        let model = model_with_id(&server.uri(), "devstral-medium-latest", false);
        let ctx = normalize_context(&Context {
            system_prompt: None,
            messages: vec![user_msg("Hi")],
            tools: Some(vec![tool("lookup")]),
        });
        let (_, _, body, _) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert_eq!(
            body["tools"],
            json!([{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "description": "Look something up",
                    "parameters": {"type": "object", "properties": {"query": {"type": "string"}}},
                    "strict": false,
                },
            }]),
            "{body}"
        );
    }

    // ---- 11. malformed stream events ----

    #[tokio::test]
    async fn non_object_stream_event_surfaces_the_upstream_message() {
        let server = wiremock::MockServer::start().await;
        let body = "data: [1,2]\n\ndata: [DONE]\n\n";
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;
        let model = model(&server.uri());
        let ctx = ctx_with(vec![user_msg("hello")]);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Invalid Mistral streaming event")
        );
    }

    #[tokio::test]
    async fn stream_without_finish_reason_is_an_error() {
        let server = wiremock::MockServer::start().await;
        mount(
            &server,
            &[json!({
                "id": "response-1",
                "choices": [{"index": 0, "finish_reason": null, "delta": {"content": "partial"}}],
            })],
        )
        .await;
        let model = model(&server.uri());
        let ctx = ctx_with(vec![user_msg("hello")]);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Mistral stream ended without a finish reason")
        );
        assert_eq!(error.stop_reason, crate::ai::types::StopReason::Error);
        // The partial text survives the error.
        assert_eq!(error.content.len(), 1);
    }

    // ---- 12. auth resolution (ambient auth is a named error (gcloud CLI out of scope); the provider
    //          config is the port wiring per the M2c ruling) ----

    #[tokio::test]
    async fn missing_api_key_is_a_lone_error_event() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[terminal_event("stop")]).await;
        let model = model(&server.uri());
        let ctx = ctx_with(vec![user_msg("hello")]);
        let api = MistralConversations;
        let mut request_cfg = cfg();
        request_cfg.api_key = String::new();
        let mut rx = api.stream(&request_cfg, &model, &ctx, &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("No API key for provider: mistral")
        );
        assert_eq!(events.len(), 1);
        assert!(apply_all(&events).is_terminal());
    }

    #[tokio::test]
    async fn options_api_key_beats_provider_config() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[terminal_event("stop")]).await;
        let model = model(&server.uri());
        let ctx = ctx_with(vec![user_msg("hello")]);
        let options = StreamOptions {
            api_key: Some("options-key".to_string()),
            ..StreamOptions::default()
        };
        let (_, headers, _, events) = capture_stream(&server, &model, &ctx, &options).await;
        assert_eq!(
            body_of(&headers, "authorization"),
            Some("Bearer options-key")
        );
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    // ---- 13. retry seam ----

    #[tokio::test]
    async fn retries_a_429_before_the_first_stream_byte() {
        let server = wiremock::MockServer::start().await;

        /// First response is an HTTP error, second (and later) succeed.
        struct Flaky {
            attempts: std::sync::atomic::AtomicU32,
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
                    sse(&[terminal_event("stop")])
                }
            }
        }

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(Flaky {
                attempts: std::sync::atomic::AtomicU32::new(0),
            })
            .mount(&server)
            .await;
        let model = model(&server.uri());
        let ctx = ctx_with(vec![user_msg("hello")]);
        let options = StreamOptions {
            max_retries: Some(1),
            ..StreamOptions::default()
        };
        let events = collect_stream(&server, &model, &ctx, &options).await;
        assert_eq!(event_types(&events), ["start", "done"], "{events:?}");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    // ---- 14. non-image models replace images with placeholders ----

    #[tokio::test]
    async fn non_image_models_get_image_placeholders() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[terminal_event("stop")]).await;
        let model = model(&server.uri()); // input: [Text]
        let ctx = ctx_with(vec![
            Message::User(UserMessage {
                content: StringOrBlocks::Blocks(vec![
                    crate::ai::types::TextOrImageBlock::Text(TextContent {
                        text: "describe".to_string(),
                        text_signature: None,
                    }),
                    crate::ai::types::TextOrImageBlock::Image(image("aGVsbG8=")),
                ]),
                timestamp: TS,
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "abc123456".to_string(),
                tool_name: "lookup".to_string(),
                content: vec![
                    crate::ai::types::TextOrImageBlock::Text(TextContent {
                        text: "found".to_string(),
                        text_signature: None,
                    }),
                    crate::ai::types::TextOrImageBlock::Image(image("aGVsbG8=")),
                ],
                details: None,
                usage: None,
                is_error: false,
                timestamp: TS,
            }),
        ]);
        let (_, _, body, _) =
            capture_stream(&server, &model, &ctx, &StreamOptions::default()).await;
        assert_eq!(
            body["messages"][0],
            json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "describe"},
                    {"type": "text", "text": "(image omitted: model does not support images)"},
                ],
            }),
            "{body}"
        );
        assert_eq!(
            body["messages"][1],
            json!({
                "role": "tool",
                "tool_call_id": "abc123456",
                "name": "lookup",
                "content": [{
                    "type": "text",
                    "text": "found\n(tool image omitted: model does not support images)",
                }],
            }),
            "{body}"
        );
    }

    // ---- 15. tool-call id derivation + normalizer (pure) ----

    #[test]
    fn derive_mistral_tool_call_id_matches_upstream_rules() {
        // A 9-char alphanumeric id passes through at attempt 0.
        assert_eq!(derive_mistral_tool_call_id("abc-123-def", 0), "abc123def");
        // The synthetic streaming id: "toolcall:0" normalizes to 9 chars.
        assert_eq!(derive_mistral_tool_call_id("toolcall:0", 0), "toolcall0");
        // Non-9-char ids fall back to shortHash (pinned against the JS
        // shortHash port already covered by the openai-completions tests).
        let derived = derive_mistral_tool_call_id("call_12345", 0);
        assert_eq!(derived.chars().count(), 9);
        assert!(derived.chars().all(|c| c.is_ascii_alphanumeric()));
        // Attempt suffix changes the seed and therefore the output.
        assert_ne!(
            derive_mistral_tool_call_id("call_12345", 0),
            derive_mistral_tool_call_id("call_12345", 1)
        );
    }

    #[test]
    fn normalizer_is_stable_and_collision_free() {
        let mut normalizer = MistralToolCallIdNormalizer::default();
        let first = normalizer.normalize("abc-123-def");
        let again = normalizer.normalize("abc-123-def");
        assert_eq!(first, "abc123def");
        assert_eq!(first, again);
        // A different id never maps onto an owned candidate.
        let mut normalizer = MistralToolCallIdNormalizer::default();
        let a = normalizer.normalize("toolcall:0");
        let b = normalizer.normalize("toolcall:1");
        assert_ne!(a, b);
        assert_eq!(a, "toolcall0");
    }

    // ---- 16. usage accounting guards ----

    #[test]
    fn cached_prompt_tokens_read_every_provider_key_shape() {
        let prompt = 10u64;
        let cached = |usage: Value| cached_prompt_tokens(&usage, prompt);
        assert_eq!(
            cached(json!({"prompt_tokens_details": {"cached_tokens": 3}})),
            3
        );
        assert_eq!(
            cached(json!({"promptTokensDetails": {"cachedTokens": 4}})),
            4
        );
        assert_eq!(
            cached(json!({"prompt_token_details": {"cached_tokens": 5}})),
            5
        );
        assert_eq!(
            cached(json!({"promptTokenDetails": {"cachedTokens": 6}})),
            6
        );
        assert_eq!(cached(json!({"numCachedTokens": 7})), 7);
        assert_eq!(cached(json!({"num_cached_tokens": 8})), 8);
        // First non-null shape wins; missing/invalid shapes degrade to 0 and
        // the result clamps to [0, promptTokens].
        assert_eq!(
            cached(json!({"prompt_tokens_details": {"cached_tokens": null}, "numCachedTokens": 9})),
            9
        );
        assert_eq!(cached(json!({})), 0);
        assert_eq!(
            cached(json!({"prompt_tokens_details": {"cached_tokens": true}})),
            0
        );
        assert_eq!(
            cached_prompt_tokens(&json!({"num_cached_tokens": 99}), 10),
            10
        );
    }

    // ---- abort surface ----

    fn signal_options(signal: Option<CancellationToken>) -> StreamOptions {
        StreamOptions {
            signal,
            ..StreamOptions::default()
        }
    }

    /// A pre-cancelled signal fails the request setup (the retry seam's
    /// `"Request aborted"` abort error) before `Start`, and the catch block
    /// settles `stopReason: "aborted"`.
    #[tokio::test]
    async fn pre_aborted_request_settles_aborted_before_start() {
        let server = wiremock::MockServer::start().await;
        let token = CancellationToken::new();
        token.cancel();
        let api = MistralConversations;
        let mut rx = api.stream(
            &cfg(),
            &model(&server.uri()),
            &ctx_with(vec![user_msg("hi")]),
            &signal_options(Some(token)),
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
    /// stream aborted with `"Request was aborted"` (upstream `readMistralEvents`
    /// abort check + catch block).
    #[tokio::test]
    async fn mid_stream_cancellation_settles_the_stream_aborted() {
        let base_url = stalled_sse_server().await;
        let token = CancellationToken::new();
        let api = MistralConversations;
        let mut rx = api.stream(
            &cfg(),
            &model(&base_url),
            &ctx_with(vec![user_msg("hi")]),
            &signal_options(Some(token.clone())),
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
