//! Google Vertex AI endpoint — full port of the stream/streamSimple
//! implementations from upstream `packages/ai/src/api/google-vertex.ts`
//! (553 lines): the `generateContentStream` request path, the express-mode
//! auth shape, the `alt=sse` wire protocol, the streaming block state machine
//! (text/thinking/toolCall), raw finish-reason preservation, the usage/cost
//! accounting, and the terminal framing. The stream processing is shared with
//! the google-generative-ai adapter upstream (google-shared.ts) and in the
//! port (`google_shared`); this module differs in URL shape and auth.
//!
//! Wire shape: the port reproduces the pinned `@google/genai` SDK 2.21.0
//! Vertex request path directly (dist/node `index.cjs`):
//! - `POST {base}/{version}/{model-path}:streamGenerateContent?alt=sse` where
//!   the model path follows the SDK's Vertex `tModel`: ids prefixed with
//!   `publishers/`, `projects/`, or `models/` ride verbatim; `vendor/model`
//!   becomes `publishers/{vendor}/models/{model}` (first two `/`-separated
//!   segments, like the SDK's `split('/', 2)`); a bare id becomes
//!   `publishers/google/models/{id}`; `..`/`?`/`&` reject with
//!   `invalid model parameter`.
//! - Express API-key mode (upstream `createClientWithApiKey`: `vertexai: true,
//!   apiKey, apiVersion: "v1"`, no project/location): the SDK base is
//!   `https://aiplatform.googleapis.com/` and no `projects/{p}/locations/{l}`
//!   prefix is prepended (the client has no project). Result:
//!   `https://aiplatform.googleapis.com/v1/publishers/google/models/{model}:streamGenerateContent?alt=sse`.
//! - Custom `model.baseUrl` (upstream `buildHttpOptions`): sets
//!   `httpOptions.baseUrl` plus `baseUrlResourceScope: COLLECTION`, which
//!   suppresses the project/location prefix; a base containing the pi
//!   `{location}` placeholder is ignored (falls back to the express default),
//!   and the appended version becomes `""` when a `v<digits>[beta<digits>]`
//!   path segment is already present.
//! - Auth is the `x-goog-api-key` header (the SDK's `NodeAuth.addKeyHeader`).
//! - Body (via `generateContentParametersToVertex`): identical to the
//!   generative-ai converter for everything pi sets — `contents`, an always
//!   present `generationConfig` (`{}` when nothing is set; carries
//!   `temperature`, `maxOutputTokens`, `thinkingConfig`), `systemInstruction`
//!   (string → `{role: "user", parts: [{text}]}` via `tContent`), `tools`,
//!   and `toolConfig` hoisted to the top level — with the model riding the
//!   URL, never the body.
//! - The regional ADC bases (`https://{location}-aiplatform.googleapis.com`,
//!   the `us`/`eu` multi-regional `https://aiplatform.{location}.rep.googleapis.com`,
//!   and global `https://aiplatform.googleapis.com`) plus the
//!   `projects/{p}/locations/{l}` URL prefix and the `Authorization: Bearer`
//!   header belong to the ADC auth path (see the deviation below).
//!
//! Deviations from upstream, all structural (mirroring the sibling ports):
//! - `GoogleVertexOptions` extension fields (`toolChoice`, `thinking`,
//!   `project`, `location`) have no port option surface on `StreamOptions`:
//!   direct `stream` calls pass none, and `streamSimple` derives the
//!   thinking/toolChoice extensions from the provider-neutral fields —
//!   carried on the internal [`GoogleOptions`] so the pure builder and the
//!   wire stay testable.
//! - Ambient auth (ADC): the port reproduces upstream's ADC **resolution**
//!   chain — the express key's `gcp-vertex-credentials` marker and `<...>`
//!   placeholders select ADC exactly like upstream `resolveApiKey`, then
//!   `resolveProject` (scoped `options.env`/`GOOGLE_CLOUD_PROJECT`/
//!   `GCLOUD_PROJECT`) and `resolveLocation` (scoped `GOOGLE_CLOUD_LOCATION`)
//!   run with the upstream messages, and `buildGoogleAuthOptions` reads the
//!   `GOOGLE_APPLICATION_CREDENTIALS` key file — all three consulting the
//!   scoped env before the process env, like upstream. The credential
//!   materialization behind it is ported for service-account key files
//!   ([`crate::ai::auth::google_adc`]: RS256 JWT assertion → token
//!   exchange → `Authorization: Bearer`, cached until the 5-minute
//!   eager-refresh threshold), and the ADC URL follows the SDK's
//!   location-based base selection with the `projects/{p}/locations/{l}`
//!   prefix. The gcloud variant stays a named error: with
//!   `GOOGLE_APPLICATION_CREDENTIALS` unset — or pointing at an
//!   `authorized_user` file, which IS `gcloud auth application-default
//!   login` state — there is no port implementation (gcloud CLI invocation
//!   is out of scope), and the branch fails with the named "not supported by
//!   this port" message instead. Explicit `Authorization` headers ride via
//!   `options.headers` and reach the wire.
//! - streamSimple error precedence: upstream vertex `streamSimple` resolves
//!   the thinking level map BEFORE `stream` runs, so a map error fires before
//!   the express-key/ADC errors — the inverse of the generative-ai adapter,
//!   whose key check lives inside streamSimple (lines 309-312) ahead of the
//!   map resolution. The port mirrors each adapter's own order.
//! - `options.signal` aborts and `fetch` injection (upstream throws "Custom
//!   fetch is not supported by the Google Vertex adapter" — no port surface),
//!   and direct-`stream` extension fields have no port surface (M2a options
//!   omission); the two abort checks and the catch block's `"aborted"` branch
//!   are unreachable. `onPayload` (google-vertex.ts:105) rides the port's
//!   process-local `RequestCallbacks`: awaited before the send, `undefined`
//!   keeps the built params, any other value (including `null`) replaces
//!   them, and a hook rejection takes the catch path. Upstream has no
//!   onResponse call site — the response hook is accepted but never invoked.
//! - The stream chunk `finishReason` handling (raw string preserved, unknown
//!   values abort with `Unhandled stop reason: {raw}`), the SDK error-chunk
//!   and HTTP error rendering, surrogate sanitization (no-op in Rust),
//!   negative-count saturation, and the unretried deterministic URL errors
//!   match the google-generative-ai port (same SDK layer); see that module's
//!   docs for the full list.
//! - `x-goog-api-client` (Node-version telemetry) and the SDK's default
//!   `google-genai-sdk/...` User-Agent are omitted; pi's UA override is sent.
//! - Upstream `getGoogleBudget` knows only the 2.5-pro and 2.5-flash families
//!   on Vertex (no flash-lite table — unlike the generative-ai adapter);
//!   every other id gets the `-1` dynamic-thinking budget.

#[cfg(test)]
mod callback_tests;

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::ai::api::azure_openai_responses::get_provider_env_value;
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
use crate::ai::types::options::{ProviderEnv, SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{StopReason, ThinkingBudgets, ToolChoice, Usage};
use crate::ai::types::Model;
use crate::ai::{now_ms, ProviderConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The API id stamped on every emitted message.
const API: &str = "google-vertex";

/// Upstream `API_VERSION` (google-vertex.ts:62): the `apiVersion` handed to
/// the SDK client; `""` when a custom base URL already carries a version.
const API_VERSION: &str = "v1";

/// SDK express/global default base (`https://aiplatform.googleapis.com/`);
/// used when the model carries no usable custom base URL. Regional
/// `{location}-aiplatform` bases need project/location (ADC, M2d).
const DEFAULT_BASE_URL: &str = "https://aiplatform.googleapis.com";

/// Upstream `GCP_VERTEX_CREDENTIALS_MARKER` (google-vertex.ts:63): pi
/// generates this options key when the user has Vertex ADC credentials
/// configured; it selects the ADC client, never express auth.
const GCP_VERTEX_CREDENTIALS_MARKER: &str = "gcp-vertex-credentials";

/// Counter for generating unique tool call ids (upstream `toolCallCounter`,
/// rendered as `{name}_{Date.now()}_{++counter}`).
static TOOL_CALL_COUNTER: AtomicU32 = AtomicU32::new(0);

// =============================================================================
// Options (google-vertex.ts:51-60)
// =============================================================================

/// Upstream `GoogleVertexOptions.thinking`.
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

/// Upstream `GoogleVertexOptions` (extension fields over the base options):
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

pub struct GoogleVertex;

impl ApiImpl for GoogleVertex {
    // Upstream wires `options?.onPayload?.(params, model)`
    // (google-vertex.ts:105); it has no onResponse call site, so only the
    // payload hook is supported here.
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
        // Upstream line 74: collapse system messages before the request task
        // (Gemini-family APIs have no mid-conversation system messages).
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
        // Upstream `streamSimple` (lines 312-354): buildBaseOptions shaping
        // (context-clamped maxTokens default over the ORIGINAL context), the
        // toolChoice passthrough, and the thinking resolution. A clamped
        // "off" and a missing reasoning both disable thinking. The extension
        // resolution may fail (unsupported thinking-level map); upstream the
        // map error throws BEFORE `stream` (and its auth errors) runs.
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

/// The streamSimple-derived extension fields (upstream `GoogleVertexOptions`
/// minus the base options): `toolChoice` and `thinking`, fallible because the
/// thinking-level map can reject. Kept separate from the base options so the
/// error precedence against the auth resolution stays explicit inside the
/// task.
type GoogleExtensions = Result<(Option<String>, Option<GoogleThinkingOption>), String>;

/// Upstream `streamSimple` thinking/toolChoice resolution (lines 316-353).
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

/// Upstream `getGoogleBudget` (lines 523-553): custom budgets first, then the
/// per-family defaults matched on the RAW (not lowercased) model id, else
/// `-1` (dynamic thinking). Vertex has no flash-lite table (unlike the
/// generative-ai adapter's `getGoogleBudget`).
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
        // Upstream order: streamSimple's thinking-map resolution (lines
        // 316-353) throws before `stream` runs; inside `stream` the client
        // construction (lines 96-103: express key, else project/location)
        // precedes buildParams (line 104) and the SDK URL build (line 109).
        let (tool_choice, thinking) = extensions?;
        let google = GoogleOptions {
            stream: stream_options,
            tool_choice,
            thinking,
        };
        let auth = resolve_auth(&google.stream, &cfg).await?;
        let headers = build_headers(&model, &google);
        let params = build_params(&model, &ctx, &google)?;
        // Upstream lines 105-108: onPayload is awaited before the SDK request
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
        let url = match &auth {
            VertexAuth::Express(_) => resolve_endpoint(&model)?,
            VertexAuth::Adc {
                project, location, ..
            } => resolve_adc_endpoint(&model, project, location)?,
        };

        let response = send_stream_request(&url, &auth, &headers, &params, &google, &signal)
            .await
            .map_err(|error| error.message)?;

        // Upstream line 111: `start` after the response arrives.
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

        // Upstream lines 261-277: flush the open block.
        close_block(&mut output, current_block.take(), &tx).await;

        // Upstream line 279: the post-stream abort check precedes the
        // pending / error guards (283-291).
        if signal.is_cancelled() {
            return Err(REQUEST_WAS_ABORTED.to_string());
        }
        if output.stop_reason == StopReason::Pending {
            return Err("Google Vertex stream ended without a finish reason".to_string());
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
            // Upstream catch block (lines 295-306): the `index` cleanup is a
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
/// content index (upstream lines 124-155, 184-199, and 261-277 share this).
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
/// lines 115-259).
async fn process_chunk(
    output: &mut AssistantMessage,
    current_block: &mut Option<OpenBlock>,
    chunk: &GoogleStreamChunk,
    model: &Model,
    tx: &mpsc::Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    // Upstream lines 118: `responseId ||= chunk.responseId` — keep the first
    // non-empty one from the stream.
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

        // Upstream lines 231-237: the raw string is recorded first
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

    // Upstream lines 239-258: usage metadata replaces the running usage and
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

/// Upstream lines 122-181: text/thinking part handling with the block-kind
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

/// Upstream lines 183-227: function-call part handling — close the open
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

    // Upstream lines 203-208: generate a unique id when none provided or the
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
// Auth resolution (resolveApiKey/resolveProject/resolveLocation, lines
// 423-459; createClientWithApiKey, 374-385)
// =============================================================================

/// The express-mode credential (upstream `resolveApiKey`, lines 428-438, read
/// from `options.apiKey`; the `ProviderConfig.api_key` fallback is the port's
/// wiring per the M2c ruling). Returns `None` when the credential is absent,
/// the pi-generated `gcp-vertex-credentials` ADC marker, or a `<...>`
/// placeholder — all three mean "ADC is configured" upstream and fall to the
/// ADC client path. A trimmed marker/placeholder options key does NOT fall
/// through to the provider-config credential: upstream treats the marker as a
/// deliberate ADC selection (the config fallback only substitutes for an
/// absent options key).
fn resolve_api_key(stream_options: &StreamOptions, cfg: &ProviderConfig) -> Option<String> {
    let candidate = match stream_options.api_key.as_deref().map(str::trim) {
        Some(trimmed) if !trimmed.is_empty() => trimmed,
        _ => cfg.api_key.trim(),
    };
    if candidate.is_empty()
        || candidate == GCP_VERTEX_CREDENTIALS_MARKER
        || is_placeholder_api_key(candidate)
    {
        return None;
    }
    Some(candidate.to_string())
}

/// Upstream `/^<[^>]+>$/` (lines 436-438): a `<...>` pi placeholder.
fn is_placeholder_api_key(api_key: &str) -> bool {
    match api_key
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
    {
        Some(inner) => !inner.is_empty() && !inner.contains('>'),
        None => false,
    }
}

/// The resolved credential (upstream lines 99-103: `apiKey ?
/// createClientWithApiKey(...) : createClient(...)` — the express client vs
/// the ADC client).
#[derive(Debug, Clone)]
enum VertexAuth {
    /// Express mode: the `x-goog-api-key` header (the SDK's
    /// `NodeAuth.addKeyHeader`).
    Express(String),
    /// ADC mode: a minted access token (`Authorization: Bearer`, the SDK's
    /// `addAuthHeaders`) plus the project/location the ADC URL needs.
    Adc {
        bearer: String,
        project: String,
        location: String,
    },
}

/// Upstream lines 99-103: the express key, else the ADC client construction
/// (`resolveProject` → `resolveLocation` → the SDK's ADC credential fetch
/// via `buildGoogleAuthOptions`'s `GOOGLE_APPLICATION_CREDENTIALS`).
///
/// The ADC materialization is ported for service-account key files
/// ([`crate::ai::auth::google_adc`]: RS256 JWT assertion → token exchange →
/// cached bearer token). The gcloud variant stays a named error: with
/// `GOOGLE_APPLICATION_CREDENTIALS` unset the port has no `gcloud auth
/// application-default login`/metadata-server surface (gcloud CLI
/// invocation is out of scope), and an `authorized_user` key file IS gcloud
/// login state — both fail with the same named message. Project/location
/// resolution keeps running first (upstream `resolveProject`/`resolveLocation`
/// throw before the SDK ever looks at credentials).
async fn resolve_auth(
    stream_options: &StreamOptions,
    cfg: &ProviderConfig,
) -> Result<VertexAuth, String> {
    match resolve_api_key(stream_options, cfg) {
        Some(api_key) => Ok(VertexAuth::Express(api_key)),
        None => {
            // The resolution chain runs to completion (named errors when the
            // env surfaces are missing), then materializes the credential.
            // All three resolvers read the scoped `options.env` first, like
            // upstream.
            let env = stream_options.env.as_ref();
            let project = resolve_project(env)?;
            let location = resolve_location(env)?;
            match build_google_auth_options(env) {
                Some(key_file) => {
                    let bearer = crate::ai::auth::google_adc::adc_access_token(&key_file).await?;
                    Ok(VertexAuth::Adc {
                        bearer,
                        project,
                        location,
                    })
                }
                None => Err(crate::ai::auth::google_adc::GCLOUD_ADC_NAMED_ERROR.to_string()),
            }
        }
    }
}

/// Upstream `resolveProject` (lines 440-451): `options.project` (no port
/// option surface — the M2a options omission), then the scoped `options.env`
/// `GOOGLE_CLOUD_PROJECT`/`GCLOUD_PROJECT` lookups.
fn resolve_project(env: Option<&ProviderEnv>) -> Result<String, String> {
    get_provider_env_value("GOOGLE_CLOUD_PROJECT", env)
        .or_else(|| get_provider_env_value("GCLOUD_PROJECT", env))
        .ok_or_else(|| {
            "Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass \
             project in options."
                .to_string()
        })
}

/// Upstream `resolveLocation` (lines 453-459): `options.location` (no port
/// option surface), then the scoped `options.env` `GOOGLE_CLOUD_LOCATION`
/// lookup.
fn resolve_location(env: Option<&ProviderEnv>) -> Result<String, String> {
    get_provider_env_value("GOOGLE_CLOUD_LOCATION", env).ok_or_else(|| {
        "Vertex AI requires a location. Set GOOGLE_CLOUD_LOCATION or pass location in options."
            .to_string()
    })
}

/// Upstream `buildGoogleAuthOptions` (lines 423-427): the
/// `GOOGLE_APPLICATION_CREDENTIALS` key file handed to the SDK's ADC, when
/// set (scoped `options.env`, then the process env).
fn build_google_auth_options(env: Option<&ProviderEnv>) -> Option<String> {
    get_provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", env)
}

// =============================================================================
// URL assembly (buildHttpOptions/createClientWithApiKey, lines 374-421, plus
// the SDK's Vertex `tModel` and `constructUrl`)
// =============================================================================

/// Upstream Vertex `tModel` (`@google/genai`): the URL path segment for the
/// model. Recognized resource prefixes ride verbatim; `vendor/model` maps to
/// `publishers/{vendor}/models/{model}` using the first two `/`-separated
/// segments (the SDK's `split('/', 2)` drops the rest); a bare id gets the
/// `publishers/google/models` publisher prefix; `..`/`?`/`&` are rejected.
fn resolve_model_path(model_id: &str) -> Result<String, String> {
    if model_id.contains("..") || model_id.contains('?') || model_id.contains('&') {
        return Err("invalid model parameter".to_string());
    }
    Ok(
        if model_id.starts_with("publishers/")
            || model_id.starts_with("projects/")
            || model_id.starts_with("models/")
        {
            model_id.to_string()
        } else if model_id.contains('/') {
            let mut segments = model_id.splitn(3, '/');
            let publisher = segments.next().unwrap_or_default();
            let name = segments.next().unwrap_or_default();
            format!("publishers/{publisher}/models/{name}")
        } else {
            format!("publishers/google/models/{model_id}")
        },
    )
}

/// Upstream `resolveCustomBaseUrl` (lines 406-412): `None` when the base is
/// empty or still carries the pi `{location}` template (the SDK then builds
/// the URL itself — the express default here); otherwise the trimmed base
/// with `getRequestUrlInternal`'s trailing-slash normalization.
fn resolve_custom_base_url(base_url: &str) -> Option<String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() || trimmed.contains("{location}") {
        return None;
    }
    Some(trimmed.trim_end_matches('/').to_string())
}

/// Upstream `baseUrlIncludesApiVersion` (lines 414-421): whether any path
/// segment matches `^v\d+(?:beta\d*)?$`. `new URL` parses absolute URLs (the
/// port splits scheme/authority/path manually); the raw-token scan matches
/// the upstream regex for strings that do not parse as URLs.
fn base_url_includes_api_version(base_url: &str) -> bool {
    let without_query = base_url.split(['?', '#']).next().unwrap_or(base_url);
    let path = match without_query.split_once("://") {
        Some((_scheme, rest)) => match rest.split_once('/') {
            Some((_authority, path)) => path,
            None => return false,
        },
        None => without_query,
    };
    path.split('/').any(is_api_version_segment)
}

/// `^v\d+(?:beta\d*)?$`
fn is_api_version_segment(segment: &str) -> bool {
    let Some(rest) = segment.strip_prefix('v') else {
        return false;
    };
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return false;
    }
    match rest[digits..].strip_prefix("beta") {
        // No `beta` suffix: the whole rest must have been digits.
        None => digits == rest.len(),
        // `beta\d*` allows zero digits (`v1beta`).
        Some(tail) => tail.chars().all(|c| c.is_ascii_digit()),
    }
}

/// The request URL (upstream `createClientWithApiKey` + `buildHttpOptions` +
/// the SDK's `constructUrl`):
/// `{base}/{version}/{model-path}:streamGenerateContent?alt=sse`. With no
/// usable custom base the SDK express default applies. A custom base
/// suppresses the project/location prefix (`baseUrlResourceScope: COLLECTION`
/// — the express client has no project anyway) and the version segment
/// (`apiVersion: ""`) when it already carries one.
fn resolve_endpoint(model: &Model) -> Result<String, String> {
    let model_path = resolve_model_path(&model.id)?;
    let head = match resolve_custom_base_url(&model.base_url) {
        Some(base) if base_url_includes_api_version(&base) => base,
        Some(base) => format!("{base}/{API_VERSION}"),
        None => format!("{DEFAULT_BASE_URL}/{API_VERSION}"),
    };
    Ok(format!("{head}/{model_path}:streamGenerateContent?alt=sse"))
}

/// The SDK's ADC base selection (`_api_client.ts` constructor, the
/// project+location case): `global` → `https://aiplatform.googleapis.com`,
/// the multi-regional `us`/`eu` →
/// `https://aiplatform.{location}.rep.googleapis.com`, else the regional
/// `https://{location}-aiplatform.googleapis.com`.
fn adc_base_url(location: &str) -> String {
    match location {
        "global" => DEFAULT_BASE_URL.to_string(),
        "us" | "eu" => format!("https://aiplatform.{location}.rep.googleapis.com"),
        _ => format!("https://{location}-aiplatform.googleapis.com"),
    }
}

/// The ADC request URL (upstream `createClient` + `buildHttpOptions` + the
/// SDK's `constructUrl`/`shouldPrependVertexProjectPath` on the ADC client):
/// a custom base (pi always pairs one with
/// `baseUrlResourceScope: COLLECTION`, which suppresses the
/// `projects/{p}/locations/{l}` prefix) follows the express head rules;
/// without one the location-selected base ALWAYS carries the prefix (the
/// POST stream path never hits the models.get exemption).
fn resolve_adc_endpoint(model: &Model, project: &str, location: &str) -> Result<String, String> {
    let model_path = resolve_model_path(&model.id)?;
    let tail = format!("{model_path}:streamGenerateContent?alt=sse");
    match resolve_custom_base_url(&model.base_url) {
        Some(base) if base_url_includes_api_version(&base) => Ok(format!("{base}/{tail}")),
        Some(base) => Ok(format!("{base}/{API_VERSION}/{tail}")),
        None => Ok(format!(
            "{}/{API_VERSION}/projects/{project}/locations/{location}/{tail}",
            adc_base_url(location)
        )),
    }
}

/// Upstream `createClient`/`createClientWithApiKey` header assembly (line 398):
/// the pi User-Agent, model headers, and the caller's headers merged last (a
/// `None` value — upstream `null` — suppresses a default header).
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

/// Upstream `buildParams` (lines 461-521) folded with the SDK's
/// `generateContentParametersToVertex` body shape.
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

    // Upstream line 483: empty when there is no initial system message (the
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
    // Upstream lines 495-505: level wins over budget; disabled thinking sends
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
/// seam is the only retry layer. Auth is the express `x-goog-api-key` header,
/// or the ADC `Authorization: Bearer` header (`addAuthHeaders`). A cancelled
/// signal fails the request with the abort error.
async fn send_stream_request(
    url: &str,
    auth: &VertexAuth,
    headers: &[(String, String)],
    body: &Value,
    options: &GoogleOptions,
    signal: &CancellationToken,
) -> Result<reqwest::Response, ProviderError> {
    let mut header_map = reqwest::header::HeaderMap::new();
    match auth {
        VertexAuth::Express(api_key) => {
            let auth = reqwest::header::HeaderValue::from_str(api_key).map_err(|error| {
                ProviderError::transport(format!("Invalid x-goog-api-key header: {error}"))
            })?;
            header_map.insert("x-goog-api-key", auth);
        }
        VertexAuth::Adc { bearer, .. } => {
            let auth = reqwest::header::HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|error| {
                    ProviderError::transport(format!("Invalid Authorization header: {error}"))
                })?;
            header_map.insert("authorization", auth);
        }
    }
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
            provider: "google-vertex".to_string(),
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

    /// The streamGenerateContent path under a `{server}/v1` base: the custom
    /// base already carries the version (no second segment appended) and the
    /// bare model id gets the publisher prefix.
    fn generate_content_path() -> String {
        "/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent".to_string()
    }

    async fn mount(server: &wiremock::MockServer, chunks: &[Value]) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(generate_content_path()))
            .and(wiremock::matchers::query_param("alt", "sse"))
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
        let api = GoogleVertex;
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
        let api = GoogleVertex;
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
    async fn wire_request_hits_publishers_models_stream_generate_content() {
        let server = wiremock::MockServer::start().await;
        mount(
            &server,
            &[text_chunk("hi"), stop_chunk(), usage_metadata_chunk()],
        )
        .await;
        // The base already carries a `/v1` segment, so no second version is
        // appended and the bare model id gets the publisher prefix.
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (url, headers, body, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;

        assert!(
            url.ends_with(&format!("{}?alt=sse", generate_content_path())),
            "{url}"
        );
        // Express auth is the x-goog-api-key header; never a bearer token.
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
    async fn custom_base_without_version_segment_appends_v1() {
        let server = wiremock::MockServer::start().await;
        // A base with no version segment gets `v1` appended before the path.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(generate_content_path()))
            .and(wiremock::matchers::query_param("alt", "sse"))
            .respond_with(sse(&[text_chunk("hi"), stop_chunk()]))
            .mount(&server)
            .await;
        let model = model(&server.uri());
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (url, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        assert!(
            url.ends_with(&format!("{}?alt=sse", generate_content_path())),
            "{url}"
        );
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    #[test]
    fn default_endpoint_falls_back_to_the_express_base() {
        let model = model("");
        assert_eq!(
            resolve_endpoint(&model).unwrap(),
            "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn location_placeholder_base_uses_the_express_default() {
        // pi generates `{location}`-templated base URLs for Vertex models;
        // upstream `resolveCustomBaseUrl` ignores them (the ADC path builds
        // the URL from project/location instead), so the express default is
        // the built-in fallback for the non-ADC path.
        let model = model(
            "https://{location}-aiplatform.googleapis.com/v1/projects/my-project/locations/{location}",
        );
        assert_eq!(
            resolve_endpoint(&model).unwrap(),
            "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn base_url_version_detection_rules() {
        let path = "publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse";
        // No version segment -> `v1` appended.
        assert_eq!(
            resolve_endpoint(&model("https://proxy.example.com")).unwrap(),
            format!("https://proxy.example.com/v1/{path}")
        );
        // A recognized version segment suppresses the append (upstream
        // `apiVersion: ""`), including `beta\d*` spellings.
        assert_eq!(
            resolve_endpoint(&model("https://proxy.example.com/v1beta2")).unwrap(),
            format!("https://proxy.example.com/v1beta2/{path}")
        );
        // Not version-prefixed segments do not suppress.
        assert_eq!(
            resolve_endpoint(&model("https://proxy.example.com/av1")).unwrap(),
            format!("https://proxy.example.com/av1/v1/{path}")
        );
        // Trailing slashes are stripped before the version check and join.
        assert_eq!(
            resolve_endpoint(&model("https://proxy.example.com/v1/")).unwrap(),
            format!("https://proxy.example.com/v1/{path}")
        );
        // Upstream does not special-case a query in the base: the version
        // check reads `new URL(...).pathname` (query excluded) and the path
        // is joined afterwards, so the model path lands INSIDE the query
        // string. The port reproduces that byte-for-byte rather than
        // "fixing" it.
        assert_eq!(
            resolve_endpoint(&model("https://proxy.example.com/v1/?x=1")).unwrap(),
            "https://proxy.example.com/v1/?x=1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn model_paths_keep_vertex_prefix_rules_and_reject_unsafe_ids() {
        assert_eq!(
            resolve_model_path("publishers/google/models/gemini-2.5-flash").unwrap(),
            "publishers/google/models/gemini-2.5-flash"
        );
        assert_eq!(
            resolve_model_path("projects/p/locations/l/endpoints/1").unwrap(),
            "projects/p/locations/l/endpoints/1"
        );
        assert_eq!(
            resolve_model_path("models/gemini-2.5-flash").unwrap(),
            "models/gemini-2.5-flash"
        );
        // `vendor/model` gets the publisher prefix (first two segments only,
        // like the SDK's `split('/', 2)`).
        assert_eq!(
            resolve_model_path("anthropic/claude-x").unwrap(),
            "publishers/anthropic/models/claude-x"
        );
        assert_eq!(
            resolve_model_path("a/b/c").unwrap(),
            "publishers/a/models/b"
        );
        assert_eq!(
            resolve_model_path("gemini-2.5-flash").unwrap(),
            "publishers/google/models/gemini-2.5-flash"
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

    #[test]
    fn placeholder_api_key_detection() {
        // Upstream `/^<[^>]+>$/`.
        assert!(is_placeholder_api_key("<authenticated>"));
        assert!(is_placeholder_api_key("<project-id>"));
        assert!(!is_placeholder_api_key("<>"));
        assert!(!is_placeholder_api_key("<a>b>"));
        assert!(!is_placeholder_api_key("AIzaSyExample"));
        assert!(!is_placeholder_api_key(""));
    }

    #[tokio::test]
    async fn explicit_headers_override_the_user_agent() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi")]).await;
        let model = model(&format!("{}/v1", server.uri()));
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
    async fn bearer_token_rides_via_explicit_headers() {
        // The M2c ruling: an explicit bearer token reaches the wire through
        // the options headers (the ADC Bearer flow itself landed in M2d).
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi"), stop_chunk()]).await;
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let mut option_headers = ProviderHeaders::new();
        option_headers.insert(
            "Authorization".to_string(),
            Some("Bearer ya29.tok".to_string()),
        );
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                headers: Some(option_headers),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        let (_, headers, _, events) = capture_simple(&server, &model, &ctx, &options).await;
        assert_eq!(body_of(&headers, "authorization"), Some("Bearer ya29.tok"));
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
    }

    #[tokio::test]
    async fn options_api_key_beats_provider_config() {
        let server = wiremock::MockServer::start().await;
        let model = model(&format!("{}/v1", server.uri()));
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

    /// The oracle's ADC-fallback halves: marker and `<...>` placeholder keys
    /// resolve to "ADC configured". With the ADC env surfaces unset the
    /// branch fails at its first missing surface (project) with the upstream
    /// message — as a lone error event, with NO wire request.
    async fn assert_adc_fallback_is_a_lone_error_event(
        server: &wiremock::MockServer,
        model: &Model,
        options: &SimpleStreamOptions,
        request_cfg: &ProviderConfig,
    ) {
        // The pinned project-error message requires the ADC env surfaces to
        // be unset; other tests in this module mutate them in parallel.
        let _env = TestEnv::apply(&[], ADC_VARS);
        let api = GoogleVertex;
        let mut rx = api.stream_simple(
            request_cfg,
            model,
            &ctx_with(vec![user_msg("hello")]),
            options,
        );
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let error = error_of(&events);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(
            error.error_message.as_deref(),
            Some(
                "Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass project in options."
            )
        );
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(error.api, API);
        assert!(apply_all(&events).is_terminal());
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "no wire request may be attempted without a credential"
        );
    }

    #[tokio::test]
    async fn placeholder_options_key_falls_back_to_adc() {
        let server = wiremock::MockServer::start().await;
        let model = model(&format!("{}/v1", server.uri()));
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                api_key: Some("<authenticated>".to_string()),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        // Even with a provider-config credential present, an explicit
        // placeholder options key means "use ADC" (upstream resolveApiKey).
        assert_adc_fallback_is_a_lone_error_event(&server, &model, &options, &cfg()).await;
    }

    #[tokio::test]
    async fn marker_options_key_falls_back_to_adc() {
        let server = wiremock::MockServer::start().await;
        let model = model(&format!("{}/v1", server.uri()));
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                api_key: Some(GCP_VERTEX_CREDENTIALS_MARKER.to_string()),
                ..Default::default()
            },
            ..SimpleStreamOptions::default()
        };
        assert_adc_fallback_is_a_lone_error_event(&server, &model, &options, &cfg()).await;
    }

    #[tokio::test]
    async fn marker_config_key_falls_back_to_adc() {
        let server = wiremock::MockServer::start().await;
        let model = model(&format!("{}/v1", server.uri()));
        let mut request_cfg = cfg();
        request_cfg.api_key = GCP_VERTEX_CREDENTIALS_MARKER.to_string();
        assert_adc_fallback_is_a_lone_error_event(
            &server,
            &model,
            &SimpleStreamOptions::default(),
            &request_cfg,
        )
        .await;
    }

    #[tokio::test]
    async fn empty_config_key_falls_back_to_adc() {
        let server = wiremock::MockServer::start().await;
        let model = model(&format!("{}/v1", server.uri()));
        let mut request_cfg = cfg();
        request_cfg.api_key = String::new();
        assert_adc_fallback_is_a_lone_error_event(
            &server,
            &model,
            &SimpleStreamOptions::default(),
            &request_cfg,
        )
        .await;
    }

    /// Process env is process-global; the shared
    /// [`test_support::TestEnv`](crate::ai::api::test_support::TestEnv)
    /// serializes env-mutating tests and restores the saved values on drop
    /// (the oracle's env stubbing).
    use crate::ai::api::test_support::TestEnv;

    const ADC_VARS: &[&str] = &[
        "GOOGLE_CLOUD_PROJECT",
        "GCLOUD_PROJECT",
        "GOOGLE_CLOUD_LOCATION",
        "GOOGLE_APPLICATION_CREDENTIALS",
    ];

    /// Upstream `resolveProject`: `GOOGLE_CLOUD_PROJECT` wins over
    /// `GCLOUD_PROJECT`; missing both fails with the upstream message.
    #[test]
    fn gcloud_project_env_wins_over_gcloud_project() {
        let _env = TestEnv::apply(
            &[
                ("GOOGLE_CLOUD_PROJECT", "fresh-project"),
                ("GCLOUD_PROJECT", "legacy-project"),
            ],
            &ADC_VARS[2..],
        );
        assert_eq!(resolve_project(None).unwrap(), "fresh-project".to_string());
    }

    #[test]
    fn missing_project_reports_the_upstream_message() {
        let _env = TestEnv::apply(&[], ADC_VARS);
        assert_eq!(
            resolve_project(None).unwrap_err(),
            "Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass \
             project in options."
                .to_string()
        );
    }

    #[test]
    fn missing_location_reports_the_upstream_message() {
        let _env = TestEnv::apply(&[("GOOGLE_CLOUD_PROJECT", "p")], &ADC_VARS[2..]);
        assert_eq!(
            resolve_location(None).unwrap_err(),
            "Vertex AI requires a location. Set GOOGLE_CLOUD_LOCATION or pass location in \
             options."
                .to_string()
        );
    }

    /// Upstream reads `options?.env` first in all three ADC resolvers: a
    /// scoped env carrying `GOOGLE_CLOUD_PROJECT`/`GOOGLE_CLOUD_LOCATION`
    /// resolves where an empty ambient env would error (the sibling bedrock
    /// port plumbs the same surface). Scoped values also win over ambient
    /// ones (`getProviderEnvValue`).
    #[tokio::test]
    async fn scoped_env_resolves_the_adc_surfaces() {
        // Ambient ADC vars cleared for the whole test: the scoped values
        // must be what resolves.
        let _env = TestEnv::apply(&[], ADC_VARS);
        let scoped = [
            (
                "GOOGLE_CLOUD_PROJECT".to_string(),
                "scoped-project".to_string(),
            ),
            (
                "GOOGLE_CLOUD_LOCATION".to_string(),
                "scoped-location".to_string(),
            ),
        ]
        .into_iter()
        .collect::<crate::ai::types::options::ProviderEnv>();
        assert_eq!(
            resolve_project(Some(&scoped)).unwrap(),
            "scoped-project".to_string()
        );
        assert_eq!(
            resolve_location(Some(&scoped)).unwrap(),
            "scoped-location".to_string()
        );

        // End to end through `resolve_auth`: the scoped env carries the ADC
        // chain past project/location; with `GOOGLE_APPLICATION_CREDENTIALS`
        // unset the gcloud variant fails with the named error.
        let stream_options = StreamOptions {
            api_key: Some(GCP_VERTEX_CREDENTIALS_MARKER.to_string()),
            env: Some(scoped),
            ..Default::default()
        };
        assert_eq!(
            resolve_auth(&stream_options, &cfg()).await.unwrap_err(),
            crate::ai::auth::google_adc::GCLOUD_ADC_NAMED_ERROR
        );
    }

    /// The completed ADC chain: with project/location resolved, the branch
    /// materializes the credential. No `GOOGLE_APPLICATION_CREDENTIALS`
    /// → the named gcloud error (no gcloud CLI surface); an unreadable key
    /// file → the minting layer's IO message naming the path. A real express
    /// key never reaches the ADC branch.
    #[tokio::test]
    async fn adc_chain_materializes_the_key_file_credential() {
        {
            let _env = TestEnv::apply(
                &[
                    ("GOOGLE_CLOUD_PROJECT", "test-project"),
                    ("GOOGLE_CLOUD_LOCATION", "us-central1"),
                ],
                &[ADC_VARS[3]],
            );
            let stream_options = StreamOptions {
                api_key: Some(GCP_VERTEX_CREDENTIALS_MARKER.to_string()),
                ..Default::default()
            };
            assert_eq!(
                resolve_auth(&stream_options, &cfg()).await.unwrap_err(),
                crate::ai::auth::google_adc::GCLOUD_ADC_NAMED_ERROR
            );
        }

        // With the key-file env set, the branch mints: a nonexistent file
        // surfaces the minting layer's IO error (the old "not supported by
        // this port" gap is gone).
        {
            let _env = TestEnv::apply(
                &[
                    ("GOOGLE_CLOUD_PROJECT", "test-project"),
                    ("GOOGLE_CLOUD_LOCATION", "us-central1"),
                    ("GOOGLE_APPLICATION_CREDENTIALS", "/home/u/sa.json"),
                ],
                &[],
            );
            let stream_options = StreamOptions {
                api_key: Some(GCP_VERTEX_CREDENTIALS_MARKER.to_string()),
                ..Default::default()
            };
            assert!(
                resolve_auth(&stream_options, &cfg())
                    .await
                    .unwrap_err()
                    .starts_with(
                        "Could not read the GOOGLE_APPLICATION_CREDENTIALS key file \
                         \"/home/u/sa.json\":"
                    ),
                "expected the minting IO error"
            );
        }

        // A real express key never reaches the ADC branch.
        let stream_options = StreamOptions {
            api_key: Some("AIzaSyExampleRealisticLookingApiKey123456".to_string()),
            ..Default::default()
        };
        match resolve_auth(&stream_options, &cfg()).await.unwrap() {
            VertexAuth::Express(api_key) => {
                assert_eq!(api_key, "AIzaSyExampleRealisticLookingApiKey123456");
            }
            other => panic!("expected the express credential, got {other:?}"),
        }
    }

    // ---- ADC service-account minting ----

    /// The SDK's ADC base selection (`_api_client.ts`): global, the
    /// `us`/`eu` multi-regional rep hosts, and the regional
    /// `{location}-aiplatform` host, always with the
    /// `projects/{p}/locations/{l}` prefix when no custom base is set.
    #[test]
    fn adc_endpoints_select_the_location_base_and_project_prefix() {
        let path = "publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse";
        let model = model("");
        assert_eq!(
            resolve_adc_endpoint(&model, "test-project", "us-central1").unwrap(),
            format!(
                "https://us-central1-aiplatform.googleapis.com/v1/projects/test-project/locations/us-central1/{path}"
            )
        );
        // global → the global endpoint (still project-scoped).
        assert_eq!(
            resolve_adc_endpoint(&model, "test-project", "global").unwrap(),
            format!(
                "https://aiplatform.googleapis.com/v1/projects/test-project/locations/global/{path}"
            )
        );
        // us/eu → the multi-regional rep hosts.
        assert_eq!(
            resolve_adc_endpoint(&model, "test-project", "us").unwrap(),
            format!(
                "https://aiplatform.us.rep.googleapis.com/v1/projects/test-project/locations/us/{path}"
            )
        );
        assert_eq!(
            resolve_adc_endpoint(&model, "test-project", "eu").unwrap(),
            format!(
                "https://aiplatform.eu.rep.googleapis.com/v1/projects/test-project/locations/eu/{path}"
            )
        );
    }

    /// A custom base suppresses the project/location prefix
    /// (`baseUrlResourceScope: COLLECTION`) and follows the express version
    /// rules; the `{location}` placeholder base is ignored like upstream.
    #[test]
    fn adc_custom_bases_follow_the_express_head_rules() {
        let path = "publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse";
        // Versioned custom base: no `v1` appended, no project prefix.
        assert_eq!(
            resolve_adc_endpoint(
                &model("https://proxy.example.com/v1"),
                "test-project",
                "us-central1"
            )
            .unwrap(),
            format!("https://proxy.example.com/v1/{path}")
        );
        // Unversioned custom base: `v1` appended, still no project prefix.
        assert_eq!(
            resolve_adc_endpoint(
                &model("https://proxy.example.com"),
                "test-project",
                "us-central1"
            )
            .unwrap(),
            format!("https://proxy.example.com/v1/{path}")
        );
        // The pi `{location}` template is ignored → the regional default
        // with the project prefix.
        assert_eq!(
            resolve_adc_endpoint(
                &model("https://{location}-aiplatform.googleapis.com/v1"),
                "test-project",
                "europe-west4"
            )
            .unwrap(),
            format!(
                "https://europe-west4-aiplatform.googleapis.com/v1/projects/test-project/locations/europe-west4/{path}"
            )
        );
    }

    /// A service-account key file in `GOOGLE_APPLICATION_CREDENTIALS` mints
    /// a bearer token (RS256 JWT exchange over the wire) and the stream
    /// request carries `Authorization: Bearer` — never `x-goog-api-key`.
    /// The minted token is cached: a second stream reuses it without a
    /// second token exchange.
    #[tokio::test]
    async fn adc_service_account_mints_a_bearer_and_streams_with_it() {
        let server = wiremock::MockServer::start().await;

        // The service-account key file, token_uri pointed at the mock.
        let mut rng = rand::rng();
        let private_key = rsa::RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let private_key_pem = {
            use rsa::pkcs8::EncodePrivateKey;
            private_key
                .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
                .unwrap()
                .to_string()
        };
        let key_dir = tempfile::tempdir().unwrap();
        let key_file = key_dir.path().join("sa.json");
        std::fs::write(
            &key_file,
            json!({
                "type": "service_account",
                "project_id": "test-project",
                "private_key": private_key_pem,
                "client_email": "sa@test-project.iam.gserviceaccount.com",
                "token_uri": format!("{}/token", server.uri()),
            })
            .to_string(),
        )
        .unwrap();

        // The token endpoint and the SSE endpoint on the same server.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        json!({"access_token": "minted-adc-test-token", "expires_in": 3600, "token_type": "Bearer"})
                            .to_string(),
                    ),
            )
            .mount(&server)
            .await;
        mount(&server, &[text_chunk("hi"), stop_chunk()]).await;

        // The whole ADC env in ONE TestEnv::apply (the guard holds ENV_LOCK
        // for the test's lifetime; a second apply would self-deadlock): the
        // three ADC surfaces set, the leftover `GCLOUD_PROJECT` cleared.
        let _env = TestEnv::apply(
            &[
                ("GOOGLE_CLOUD_PROJECT", "test-project"),
                ("GOOGLE_CLOUD_LOCATION", "us-central1"),
                ("GOOGLE_APPLICATION_CREDENTIALS", key_file.to_str().unwrap()),
            ],
            &["GCLOUD_PROJECT"],
        );
        // An empty provider-config credential so the express key never
        // resolves and the branch selects ADC.
        let mut request_cfg = cfg();
        request_cfg.api_key = String::new();

        let api = GoogleVertex;
        let run_stream = || {
            let server = &server;
            let request_cfg = &request_cfg;
            let api = &api;
            async move {
                let model = model(&format!("{}/v1", server.uri()));
                let mut rx = api.stream_simple(
                    request_cfg,
                    &model,
                    &ctx_with(vec![user_msg("hello")]),
                    &SimpleStreamOptions::default(),
                );
                let mut events = Vec::new();
                while let Some(event) = rx.recv().await {
                    events.push(event);
                }
                events
            }
        };

        let events = run_stream().await;
        assert!(
            matches!(events.last(), Some(AssistantMessageEvent::Done { .. })),
            "{events:?}"
        );

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2, "one token exchange + one stream");
        assert_eq!(requests[0].url.path(), "/token");
        assert_eq!(requests[1].url.path(), generate_content_path());
        let headers = &requests[1].headers;
        assert_eq!(
            body_of(headers, "authorization"),
            Some("Bearer minted-adc-test-token")
        );
        assert_eq!(body_of(headers, "x-goog-api-key"), None);

        // The second stream reuses the cached token (still one /token hit).
        let events = run_stream().await;
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 3, "cache hit: no second token exchange");
        assert_eq!(
            requests.iter().filter(|r| r.url.path() == "/token").count(),
            1
        );
    }

    /// Upstream vertex streamSimple resolves the thinking-level map BEFORE
    /// `stream` (and its auth errors) runs — the inverse order of the
    /// generative-ai adapter, where the key check lives in streamSimple.
    #[tokio::test]
    async fn thinking_map_error_precedes_the_adc_fallback() {
        let server = wiremock::MockServer::start().await;
        let model = Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            thinking_level_map: Some(level_map(&[("xhigh", Some("extreme"))])),
            ..model_with_id(&format!("{}/v1", server.uri()), "gemini-2.5-flash")
        };
        let ctx = ctx_with(vec![user_msg("hi")]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Xhigh),
            ..SimpleStreamOptions::default()
        };
        let mut request_cfg = cfg();
        request_cfg.api_key = String::new();
        let api = GoogleVertex;
        let mut rx = api.stream_simple(&request_cfg, &model, &ctx, &options);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some(
                "Unsupported Google thinking level mapping for google-vertex/gemini-2.5-flash: xhigh -> extreme"
            )
        );
        assert_eq!(events.len(), 1);
        assert!(apply_all(&events).is_terminal());
    }

    #[tokio::test]
    async fn invalid_model_id_surfaces_the_sdk_error_message() {
        let server = wiremock::MockServer::start().await;
        let model = model_with_id(&format!("{}/v1", server.uri()), "bad?model");
        let ctx = ctx_with(vec![user_msg("hello")]);
        let events = collect_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("invalid model parameter")
        );
        assert_eq!(events.len(), 1, "lone error event: {events:?}");
        assert_eq!(error.api, API);
        assert_eq!(error.provider, "google-vertex");
        assert!(apply_all(&events).is_terminal());
    }

    // ---- 2. request body fields ----

    #[tokio::test]
    async fn temperature_and_max_tokens_fill_generation_config() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("hi"), stop_chunk()]).await;
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model_with_id(&format!("{}/v1", server.uri()), "gemini-3-flash-preview");
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
        let thinking =
            capture_thinking_config(&server, &model, &SimpleStreamOptions::default()).await;
        assert_eq!(thinking, json!({"thinkingBudget": 0}));
    }

    #[tokio::test]
    async fn disabled_thinking_sends_budget_zero_for_gemini_3() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = model_with_id(&format!("{}/v1", server.uri()), "gemini-3-flash-preview");
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
        let mut model = model_with_id(&format!("{}/v1", server.uri()), "gemini-3.8-flash");
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
        let model = model_with_id(&format!("{}/v1", server.uri()), "gemini-3.1-pro-preview");
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
    async fn custom_budgets_override_mapped_levels() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let model = Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            thinking_level_map: Some(level_map(&[("xhigh", Some("high"))])),
            ..model_with_id(&format!("{}/v1", server.uri()), "gemini-2.5-flash")
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
    async fn default_budgets_follow_the_vertex_model_families() {
        let server = wiremock::MockServer::start().await;
        mount(&server, &[text_chunk("pong")]).await;
        let base = format!("{}/v1", server.uri());
        // Vertex's getGoogleBudget has only the 2.5-pro and 2.5-flash tables:
        // there is no dedicated flash-lite entry (unlike the generative-ai
        // adapter), and `includes("2.5-flash")` matches flash-lite ids too,
        // so they inherit the flash table.
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
            ..model_with_id(&format!("{}/v1", server.uri()), "gemini-2.5-flash")
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
                "Unsupported Google thinking level mapping for google-vertex/gemini-2.5-flash: xhigh -> extreme"
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Google Vertex stream ended without a finish reason")
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
        let ctx = ctx_with(vec![user_msg("hello")]);
        let (_, _, _, events) =
            capture_simple(&server, &model, &ctx, &SimpleStreamOptions::default()).await;
        let error = error_of(&events);
        assert_eq!(error.stop_reason, StopReason::Error);
        // The raw string is recorded before the throw (upstream line 232).
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let model = model(&format!("{}/v1", server.uri()));
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
        let api = GoogleVertex;
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
    /// stream aborted with `"Request was aborted"` (upstream line 279).
    #[tokio::test]
    async fn mid_stream_cancellation_settles_the_stream_aborted() {
        let base_url = stalled_sse_server().await;
        let token = CancellationToken::new();
        let options = StreamOptions {
            signal: Some(token.clone()),
            ..StreamOptions::default()
        };
        let api = GoogleVertex;
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
