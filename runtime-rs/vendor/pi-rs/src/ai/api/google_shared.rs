//! Shared protocol for the Google Generative AI and Google Vertex APIs —
//! full port of upstream `packages/ai/src/api/google-shared.ts` (515 lines):
//! the transcript → Gemini `Content[]` conversion, the tool-declaration
//! conversion (full JSON Schema via `parametersJsonSchema`, or OpenAPI-stripped
//! via `parameters`), the thinking-level machinery (`thinkingLevel` wire
//! control vs `thinkingBudget`), thought-signature handling, the function
//! calling-mode resolution, the stop-reason mappers, and the retry seam both
//! Google endpoints (M2c Tasks 5-6) wrap their requests in.
//!
//! Deviations from upstream, all structural:
//! - `sanitizeSurrogates` is a no-op: Rust `String` is UTF-8 and cannot hold
//!   unpaired surrogates (same disclosure as `openai_responses_shared`).
//! - The `@google/genai` SDK types (`Part`, `Content`, `ThinkingConfig`,
//!   `FinishReason`, `FunctionCallingConfigMode`, `ThinkingLevel`) are ported
//!   as the [`GooglePart`]/[`GoogleContent`]/[`GoogleThinkingConfig`]/
//!   [`GoogleFinishReason`]/[`GoogleFunctionCallingConfigMode`]
//!   [`GoogleApiThinkingLevel`] wire types, serialized with the SDK's exact
//!   enum strings. `GoogleSdkThinkingLevel` had the same values as
//!   `GoogleApiThinkingLevel`, so [`to_google_sdk_thinking_level`] is the
//!   identity and both upstream functions are preserved.
//! - `retryGoogleRequest`'s ApiError normalization (adding a missing
//!   `headers` property so `retryProviderRequest` sees one) is moot in Rust:
//!   the port's [`ProviderError`] always carries `headers` as an `Option`,
//!   and `is_retryable_provider_error` treats `None` exactly like the
//!   headers-less SDK error upstream normalizes. The function stays as the
//!   named seam delegating to [`retry_provider_request`].
//! - JSON object key order follows `serde_json`, not JS insertion order;
//!   visible only in `serde_json::Value` payloads (`functionCall.args`,
//!   `functionResponse.response`), which carry JSON data, not order.
//! - Regexes (`/gemini-3(?:\.\d+)?-(?:pro|flash)/`, `/gemma-?4/`,
//!   `/^gemini(?:-live)?-(\d+)/`, the base64 signature pattern, the
//!   tool-call-id sanitizer) are hand-rolled scanners, no regex dependency.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use crate::ai::api::openai_completions::request::{
    clamp_thinking_level, level_key, make_strict_json_schema, map_level,
    resolve_json_schema_strict_sampling, transform_messages, MappedLevel,
};
use crate::ai::retry::{retry_provider_request, ProviderError};
use crate::ai::transcript::{
    collapse_system_messages, without_initial_system_message, TranscriptContext,
};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, TextOrImageBlock,
};
use crate::ai::types::model::ModelInput;
use crate::ai::types::primitives::{StopReason, ThinkingLevel};
use crate::ai::types::tool::Tool;
use crate::ai::types::Model;

// =============================================================================
// Thinking levels (google-shared.ts:30-111)
// =============================================================================

/// Upstream `GoogleApiThinkingLevel` (google-shared.ts:36): the Gemini
/// `thinkingLevel` wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GoogleApiThinkingLevel {
    #[serde(rename = "THINKING_LEVEL_UNSPECIFIED")]
    ThinkingLevelUnspecified,
    #[serde(rename = "MINIMAL")]
    Minimal,
    #[serde(rename = "LOW")]
    Low,
    #[serde(rename = "MEDIUM")]
    Medium,
    #[serde(rename = "HIGH")]
    High,
}

/// Upstream `ResolvedGoogleThinkingLevel` (google-shared.ts:37): a pi thinking
/// level Google accepts verbatim (`Exclude<ThinkingLevel, "xhigh" | "max">`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolvedGoogleThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

/// Upstream `resolveGoogleThinkingLevel` (google-shared.ts:48-65): resolve a
/// supported pi level or the model's Google mapping to a standard Google
/// level. Throws upstream for unsupported mappings; the error message is
/// pinned by the google-thinking-level-map oracle.
pub fn resolve_google_thinking_level(
    model: &Model,
    level: ThinkingLevel,
) -> Result<ResolvedGoogleThinkingLevel, String> {
    let key = level_key(Some(level));
    let mapped = map_level(model, key);
    let resolved = match &mapped {
        MappedLevel::Value(value) => value.to_lowercase(),
        _ => key.to_string(),
    };
    match resolved.as_str() {
        "minimal" => Ok(ResolvedGoogleThinkingLevel::Minimal),
        "low" => Ok(ResolvedGoogleThinkingLevel::Low),
        "medium" => Ok(ResolvedGoogleThinkingLevel::Medium),
        "high" => Ok(ResolvedGoogleThinkingLevel::High),
        _ => {
            // JS `String(mapped)`: undefined -> "undefined", null -> "null".
            let rendered = match &mapped {
                MappedLevel::Value(value) => value.clone(),
                MappedLevel::Null => "null".to_string(),
                MappedLevel::Absent => "undefined".to_string(),
            };
            Err(format!(
                "Unsupported Google thinking level mapping for {}/{}: {} -> {}",
                model.provider, model.id, key, rendered
            ))
        }
    }
}

/// Upstream `usesGoogleThinkingLevel` (google-shared.ts:72-83): whether the
/// model uses Gemini's discrete `thinkingLevel` control instead of the
/// token-based `thinkingBudget` control. Supported levels come from the
/// model's `thinkingLevelMap`; this only selects the Google wire format.
pub fn uses_google_thinking_level(model: &Model) -> bool {
    let id = model.id.to_lowercase();
    // Match Gemini 3 Pro/Flash IDs with or without a minor version, such as
    // gemini-3-flash-preview, gemini-3.1-pro-preview, and gemini-3.8-flash.
    if id == "gemini-flash-latest" || id == "gemini-flash-lite-latest" {
        return true;
    }
    contains_gemini3_pro_or_flash(&id) || contains_gemma4(&id)
}

/// Upstream `toGoogleThinkingLevel` (google-shared.ts:85-96).
pub fn to_google_thinking_level(level: ResolvedGoogleThinkingLevel) -> GoogleApiThinkingLevel {
    match level {
        ResolvedGoogleThinkingLevel::Minimal => GoogleApiThinkingLevel::Minimal,
        ResolvedGoogleThinkingLevel::Low => GoogleApiThinkingLevel::Low,
        ResolvedGoogleThinkingLevel::Medium => GoogleApiThinkingLevel::Medium,
        ResolvedGoogleThinkingLevel::High => GoogleApiThinkingLevel::High,
    }
}

/// Upstream `toGoogleSdkThinkingLevel` (google-shared.ts:98-100): the SDK
/// enum spellings match the API-level strings, so the port collapses the two
/// types into one and the map is the identity.
pub fn to_google_sdk_thinking_level(level: GoogleApiThinkingLevel) -> GoogleApiThinkingLevel {
    level
}

/// Upstream `ThinkingConfig` (`@google/genai`) restricted to the fields this
/// port sets: `includeThoughts`, `thinkingBudget`, `thinkingLevel`.
/// `thinkingBudget` is signed (`i64`) because the endpoints pass `-1` for
/// dynamic thinking (upstream `getGoogleBudget`'s fallback), `0` to disable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<GoogleApiThinkingLevel>,
}

/// Upstream `getDisabledGoogleThinkingConfig` (google-shared.ts:102-111): the
/// thinking config that turns thinking off — `{ thinkingBudget: 0 }` for
/// budget-controlled models, else the nearest supported discrete level (the
/// `clampThinkingLevel(model, "off")` fallback).
pub fn get_disabled_google_thinking_config(model: &Model) -> Result<GoogleThinkingConfig, String> {
    if !uses_google_thinking_level(model) {
        return Ok(GoogleThinkingConfig {
            thinking_budget: Some(0),
            ..Default::default()
        });
    }
    // Upstream `clampThinkingLevel(model, "off")`: "off" when supported, else
    // the nearest supported discrete level (the forward walk finds it).
    let fallback = clamp_thinking_level(model, None);
    let Some(fallback) = fallback else {
        return Ok(GoogleThinkingConfig {
            thinking_budget: Some(0),
            ..Default::default()
        });
    };
    let resolved_level = resolve_google_thinking_level(model, fallback)?;
    let api_level = to_google_thinking_level(resolved_level);
    Ok(GoogleThinkingConfig {
        thinking_level: Some(to_google_sdk_thinking_level(api_level)),
        ..Default::default()
    })
}

// =============================================================================
// Thought signatures (google-shared.ts:113-160)
// =============================================================================

/// Upstream `isThinkingPart` (google-shared.ts:128-130): `thought === true` is
/// the definitive thinking marker. `thoughtSignature` alone never indicates
/// thinking content — it is context-replay data that can ride any part type.
pub fn is_thinking_part(part: &GooglePart) -> bool {
    part.thought == Some(true)
}

/// Upstream `retainThoughtSignature` (google-shared.ts:141-144): keep the last
/// non-empty signature for the current streamed block — later deltas may omit
/// it, and it must not be overwritten with nothing.
pub fn retain_thought_signature(existing: Option<&str>, incoming: Option<&str>) -> Option<String> {
    match incoming {
        Some(incoming) if !incoming.is_empty() => Some(incoming.to_string()),
        _ => existing.map(str::to_string),
    }
}

/// Upstream `isValidThoughtSignature` (google-shared.ts:149-153): non-empty,
/// UTF-16 length a multiple of 4, and matching `^[A-Za-z0-9+/]+={0,2}$`.
fn is_valid_thought_signature(signature: Option<&str>) -> bool {
    let Some(signature) = signature.filter(|signature| !signature.is_empty()) else {
        return false;
    };
    if signature.encode_utf16().count() % 4 != 0 {
        return false;
    }
    is_base64_signature(signature)
}

/// JS `/^[A-Za-z0-9+/]+={0,2}$/`: at least one base64 character followed by at
/// most two trailing `=`.
fn is_base64_signature(signature: &str) -> bool {
    let body = signature.trim_end_matches('=');
    let padding = signature.len() - body.len();
    padding <= 2
        && !body.is_empty()
        && body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
}

/// Upstream `resolveThoughtSignature` (google-shared.ts:158-160): only keep
/// signatures from the same provider/model with valid base64.
fn resolve_thought_signature(
    is_same_provider_and_model: bool,
    signature: Option<&str>,
) -> Option<String> {
    if is_same_provider_and_model && is_valid_thought_signature(signature) {
        signature.map(str::to_string)
    } else {
        None
    }
}

// =============================================================================
// Gemini Content wire types (@google/genai `Content`/`Part` subset)
// =============================================================================

/// Upstream `Content.role`: the two roles Gemini accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GoogleContentRole {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "model")]
    Model,
}

/// Upstream `Content`: one turn of the Gemini conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoogleContent {
    pub role: GoogleContentRole,
    pub parts: Vec<GooglePart>,
}

/// Upstream `Part` subset the port produces and consumes: text (optionally
/// thought content), inline base64 media, function calls, and function
/// responses. `thoughtSignature` can appear on any part type.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GooglePart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<GoogleInlineData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<GoogleFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_response: Option<GoogleFunctionResponse>,
}

/// Upstream `Part.inlineData`: base64 media with its MIME type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleInlineData {
    pub mime_type: String,
    pub data: String,
}

/// Upstream `FunctionCall`: `args` is the JSON arguments object (upstream
/// `?? {}` collapses null to an empty object); `id` rides only when
/// [`requires_tool_call_id`] holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleFunctionCall {
    pub name: String,
    pub args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Upstream `FunctionResponse`: `response` is `{ output: string }` on success
/// and `{ error: string }` on failure; `parts` nests images for Gemini 3+
/// multimodal function responses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleFunctionResponse {
    pub name: String,
    pub response: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<GooglePart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

// =============================================================================
// Model-id helpers (google-shared.ts:163-186)
// =============================================================================

/// Upstream `requiresToolCallId` (google-shared.ts:165-172): models via Google
/// APIs that require explicit tool call ids in function calls/responses.
pub fn requires_tool_call_id(model_id: &str) -> bool {
    let gemini_major_version = get_gemini_major_version(model_id);
    model_id.starts_with("claude-")
        || model_id.starts_with("gpt-oss-")
        || gemini_major_version.is_some_and(|version| version >= 3)
}

/// Upstream `getGeminiMajorVersion` (google-shared.ts:174-178):
/// `^gemini(?:-live)?-(\d+)` on the lowercased id.
fn get_gemini_major_version(model_id: &str) -> Option<u32> {
    let id = model_id.to_lowercase();
    let rest = id.strip_prefix("gemini")?;
    let rest = rest.strip_prefix("-live").unwrap_or(rest);
    let rest = rest.strip_prefix('-')?;
    let digits_end = rest
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(rest.len());
    let digits = &rest[..digits_end];
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Upstream `supportsMultimodalFunctionResponse` (google-shared.ts:180-186):
/// Gemini 3+ models nest images inside `functionResponse.parts`; non-Gemini
/// models default to true.
fn supports_multimodal_function_response(model_id: &str) -> bool {
    match get_gemini_major_version(model_id) {
        Some(version) => version >= 3,
        None => true,
    }
}

/// JS `/gemini-3(?:\.\d+)?-(?:pro|flash)/` — unanchored search over the
/// lowercased id. The optional minor version is `\.\d+`: a dot followed by at
/// least one digit.
fn contains_gemini3_pro_or_flash(id: &str) -> bool {
    let bytes = id.as_bytes();
    let mut search_from = 0;
    while let Some(found) = id[search_from..].find("gemini-3") {
        let start = search_from + found;
        let mut cursor = start + "gemini-3".len();
        if bytes.get(cursor) == Some(&b'.') {
            let mut end = cursor + 1;
            while bytes.get(end).is_some_and(|byte| byte.is_ascii_digit()) {
                end += 1;
            }
            if end > cursor + 1 {
                cursor = end;
            }
        }
        if id[cursor..].starts_with("-pro") || id[cursor..].starts_with("-flash") {
            return true;
        }
        // JS regex retries at the next code unit; the scanned bytes are ASCII.
        search_from = start + 1;
    }
    false
}

/// JS `/gemma-?4/` — both hosted Gemma 4 naming forms.
fn contains_gemma4(id: &str) -> bool {
    let mut search_from = 0;
    while let Some(found) = id[search_from..].find("gemma") {
        let start = search_from + found;
        let rest = &id[start + "gemma".len()..];
        let rest = rest.strip_prefix('-').unwrap_or(rest);
        if rest.starts_with('4') {
            return true;
        }
        search_from = start + 1;
    }
    false
}

/// Upstream `normalizeToolCallId` (google-shared.ts:195-198): replace every
/// non-`[a-zA-Z0-9_-]` UTF-16 code unit with `_`, clamp to 64 code units.
fn normalize_google_tool_call_id(id: &str) -> String {
    let sanitized: String = id
        .encode_utf16()
        .map(|unit| match unit {
            0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A | 0x2D | 0x5F => {
                char::from_u32(u32::from(unit)).unwrap_or('_')
            }
            _ => '_',
        })
        .collect();
    // Every emitted char is ASCII, so chars equal UTF-16 code units and the
    // clamp is JS `slice(0, 64)`.
    sanitized.chars().take(64).collect()
}

// =============================================================================
// Message conversion (google-shared.ts:191-343)
// =============================================================================

/// Upstream `convertMessages` (google-shared.ts:191-343): internal messages to
/// Gemini `Content[]`. Gemini has no mid-conversation system messages — the
/// transcript is collapsed and the leading system message is dropped (it is
/// sent as `systemInstruction` by the endpoints).
pub fn convert_messages(model: &Model, context: &TranscriptContext) -> Vec<GoogleContent> {
    // Gemini has no mid-conversation system messages; the leading prompt is
    // sent as systemInstruction (by the endpoints).
    let conversation = without_initial_system_message(collapse_system_messages(context.clone()).0);
    let normalize_tool_call_id = |id: &str, _source: &AssistantMessage| -> String {
        if !requires_tool_call_id(&model.id) {
            return id.to_string();
        }
        normalize_google_tool_call_id(id)
    };

    let transformed = transform_messages(model, &conversation, &normalize_tool_call_id);

    let mut contents: Vec<GoogleContent> = Vec::new();
    for msg in transformed {
        match msg {
            Message::User(user) => match &user.content {
                StringOrBlocks::Text(text) => {
                    contents.push(GoogleContent {
                        role: GoogleContentRole::User,
                        parts: vec![text_part(text)],
                    });
                }
                StringOrBlocks::Blocks(blocks) => {
                    let parts: Vec<GooglePart> = blocks
                        .iter()
                        .map(|item| match item {
                            TextOrImageBlock::Text(text) => text_part(&text.text),
                            TextOrImageBlock::Image(image) => GooglePart {
                                inline_data: Some(GoogleInlineData {
                                    mime_type: image.mime_type.clone(),
                                    data: image.data.clone(),
                                }),
                                ..Default::default()
                            },
                        })
                        .collect();
                    if parts.is_empty() {
                        continue;
                    }
                    contents.push(GoogleContent {
                        role: GoogleContentRole::User,
                        parts,
                    });
                }
            },
            Message::Assistant(assistant) => {
                let mut parts: Vec<GooglePart> = Vec::new();
                // Only messages from the same provider and model keep
                // thinking blocks and signatures.
                let is_same_provider_and_model =
                    assistant.provider == model.provider && assistant.model == model.id;

                for block in &assistant.content {
                    match block {
                        AssistantBlock::Text(text_block) => {
                            let thought_signature = resolve_thought_signature(
                                is_same_provider_and_model,
                                text_block.text_signature.as_deref(),
                            );
                            // Skip empty text blocks — unless they carry a
                            // thought signature. Gemini can attach the
                            // signature to a part whose visible text is empty
                            // and requires it echoed back; dropping it breaks
                            // the reasoning chain.
                            if text_block.text.trim().is_empty() && thought_signature.is_none() {
                                continue;
                            }
                            parts.push(GooglePart {
                                text: Some(text_block.text.clone()),
                                thought_signature,
                                ..Default::default()
                            });
                        }
                        AssistantBlock::Thinking(thinking_block) => {
                            // Only keep as a thinking block when same provider
                            // AND model; otherwise convert to plain text (no
                            // tags, to avoid the model mimicking them).
                            if is_same_provider_and_model {
                                let thought_signature = resolve_thought_signature(
                                    is_same_provider_and_model,
                                    thinking_block.thinking_signature.as_deref(),
                                );
                                // Same rule as text blocks: an empty thinking
                                // block is dropped only when it carries no
                                // signature (mirrors the anthropic converter).
                                if thinking_block.thinking.trim().is_empty()
                                    && thought_signature.is_none()
                                {
                                    continue;
                                }
                                parts.push(GooglePart {
                                    thought: Some(true),
                                    text: Some(thinking_block.thinking.clone()),
                                    thought_signature,
                                    ..Default::default()
                                });
                            } else {
                                // Cross-provider/model: the signature is
                                // unusable, empty blocks stay dropped.
                                if thinking_block.thinking.trim().is_empty() {
                                    continue;
                                }
                                parts.push(text_part(&thinking_block.thinking));
                            }
                        }
                        AssistantBlock::ToolCall(call) => {
                            let thought_signature = resolve_thought_signature(
                                is_same_provider_and_model,
                                call.thought_signature.as_deref(),
                            );
                            parts.push(GooglePart {
                                function_call: Some(GoogleFunctionCall {
                                    name: call.name.clone(),
                                    args: if call.arguments.is_null() {
                                        json!({})
                                    } else {
                                        call.arguments.clone()
                                    },
                                    id: requires_tool_call_id(&model.id).then(|| call.id.clone()),
                                }),
                                thought_signature,
                                ..Default::default()
                            });
                        }
                    }
                }

                if parts.is_empty() {
                    continue;
                }
                contents.push(GoogleContent {
                    role: GoogleContentRole::Model,
                    parts,
                });
            }
            Message::ToolResult(result) => {
                // Extract text and image content.
                let text_result = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        TextOrImageBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let image_content: Vec<&crate::ai::types::content::ImageContent> =
                    if model.input.contains(&ModelInput::Image) {
                        result
                            .content
                            .iter()
                            .filter_map(|block| match block {
                                TextOrImageBlock::Image(image) => Some(image),
                                _ => None,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };

                let has_text = !text_result.is_empty();
                let has_images = !image_content.is_empty();

                // Gemini 3+ models support multimodal function responses with
                // images nested inside functionResponse.parts. Claude and
                // other non-Gemini models behind Cloud Code Assist / Gemini < 3
                // still need a separate user image turn.
                let model_supports_multimodal_function_response =
                    supports_multimodal_function_response(&model.id);

                // Use "output" key for success, "error" key for errors as per
                // SDK documentation.
                let response_value = if has_text {
                    text_result
                } else if has_images {
                    "(see attached image)".to_string()
                } else {
                    String::new()
                };

                let image_parts: Vec<GooglePart> = image_content
                    .iter()
                    .map(|image| GooglePart {
                        inline_data: Some(GoogleInlineData {
                            mime_type: image.mime_type.clone(),
                            data: image.data.clone(),
                        }),
                        ..Default::default()
                    })
                    .collect();

                let function_response_part = GooglePart {
                    function_response: Some(GoogleFunctionResponse {
                        name: result.tool_name.clone(),
                        response: if result.is_error {
                            json!({ "error": response_value })
                        } else {
                            json!({ "output": response_value })
                        },
                        parts: (has_images && model_supports_multimodal_function_response)
                            .then(|| image_parts.clone()),
                        id: requires_tool_call_id(&model.id).then(|| result.tool_call_id.clone()),
                    }),
                    ..Default::default()
                };

                // Cloud Code Assist API requires all function responses to be
                // in a single user turn: merge into the previous user turn
                // when it already carries function responses.
                let should_merge = matches!(
                    contents.last(),
                    Some(last)
                        if last.role == GoogleContentRole::User
                            && last.parts.iter().any(|part| part.function_response.is_some())
                );
                if should_merge {
                    if let Some(last) = contents.last_mut() {
                        last.parts.push(function_response_part);
                    }
                } else {
                    contents.push(GoogleContent {
                        role: GoogleContentRole::User,
                        parts: vec![function_response_part],
                    });
                }

                // For Gemini < 3, add images in a separate user message.
                if has_images && !model_supports_multimodal_function_response {
                    let mut turn_parts = vec![text_part("Tool result image:")];
                    turn_parts.extend(image_parts);
                    contents.push(GoogleContent {
                        role: GoogleContentRole::User,
                        parts: turn_parts,
                    });
                }
            }
            // Collapse already removed system messages; nothing to convert.
            Message::System(_) => {}
        }
    }

    contents
}

/// A bare text part (`{ text }`).
fn text_part(text: &str) -> GooglePart {
    GooglePart {
        text: Some(text.to_string()),
        ..Default::default()
    }
}

// =============================================================================
// Tool conversion (google-shared.ts:345-401)
// =============================================================================

/// Upstream `JSON_SCHEMA_META_DECLARATIONS` (google-shared.ts:345-354).
const JSON_SCHEMA_META_DECLARATIONS: [&str; 8] = [
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "$vocabulary",
    "$comment",
    "$defs",
    "definitions", // pre-draft-2019-09 equivalent of $defs
];

/// Upstream `sanitizeForOpenApi` (google-shared.ts:359-370): strip meta
/// declarations from a schema object, recursively — but arrays pass through
/// untouched (upstream returns them before recursing into elements).
fn sanitize_for_open_api(schema: &Value) -> Value {
    match schema {
        Value::Object(entries) => {
            let mut result = Map::new();
            for (key, value) in entries {
                if JSON_SCHEMA_META_DECLARATIONS.contains(&key.as_str()) {
                    continue;
                }
                result.insert(key.clone(), sanitize_for_open_api(value));
            }
            Value::Object(result)
        }
        // Non-objects (including arrays) pass through untouched; upstream
        // returns before recursing into array elements.
        other => other.clone(),
    }
}

/// Upstream `getJsonSchemaToolParameters`
/// (`api/constrained-sampling.ts:129-131`): the strict-converted parameters
/// when strict sampling resolved to true, the tool's parameters verbatim
/// otherwise.
fn get_json_schema_tool_parameters(tool: &Tool, strict: Option<bool>) -> Result<Value, String> {
    if strict == Some(true) {
        make_strict_json_schema(&tool.parameters, None)
    } else {
        Ok(tool.parameters.clone())
    }
}

/// Upstream `convertTools` (google-shared.ts:380-401): tools → Gemini function
/// declarations. `use_parameters` sends the OpenAPI-stripped `parameters`
/// field (Cloud Code Assist with Claude models); the default sends full JSON
/// Schema as `parametersJsonSchema`. `None` for an empty tool list.
pub fn convert_tools(
    tools: &[Tool],
    use_parameters: bool,
    supports_strict_mode: bool,
) -> Result<Option<Vec<Value>>, String> {
    if tools.is_empty() {
        return Ok(None);
    }
    let declarations = tools
        .iter()
        .map(|tool| {
            let strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode, None)?;
            let parameters = get_json_schema_tool_parameters(tool, strict)?;
            let mut declaration = Map::new();
            declaration.insert("name".into(), json!(tool.name));
            declaration.insert("description".into(), json!(tool.description));
            if use_parameters {
                declaration.insert("parameters".into(), sanitize_for_open_api(&parameters));
            } else {
                declaration.insert("parametersJsonSchema".into(), parameters);
            }
            Ok(Value::Object(declaration))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Some(vec![json!({ "functionDeclarations": declarations })]))
}

// =============================================================================
// Function calling modes (google-shared.ts:403-436)
// =============================================================================

/// Upstream `FunctionCallingConfigMode` (`@google/genai`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GoogleFunctionCallingConfigMode {
    #[serde(rename = "MODE_UNSPECIFIED")]
    ModeUnspecified,
    #[serde(rename = "AUTO")]
    Auto,
    #[serde(rename = "ANY")]
    Any,
    #[serde(rename = "NONE")]
    None,
    #[serde(rename = "VALIDATED")]
    Validated,
}

/// Upstream `supportsGoogleStrictToolSampling` (google-shared.ts:404-407):
/// Gemini 3+ enforces required function parameters in validated tool-calling
/// modes.
pub fn supports_google_strict_tool_sampling(model_id: &str) -> bool {
    get_gemini_major_version(model_id).is_some_and(|major_version| major_version >= 3)
}

/// Upstream `mapToolChoice` (google-shared.ts:410-421).
pub fn map_tool_choice(choice: &str) -> GoogleFunctionCallingConfigMode {
    match choice {
        "auto" => GoogleFunctionCallingConfigMode::Auto,
        "none" => GoogleFunctionCallingConfigMode::None,
        "any" => GoogleFunctionCallingConfigMode::Any,
        _ => GoogleFunctionCallingConfigMode::Auto,
    }
}

/// Upstream `resolveGoogleFunctionCallingMode` (google-shared.ts:423-436):
/// explicit none/any win, then VALIDATED when any tool resolves to strict,
/// then the mapped choice, else `None`. A require-strict tool over an
/// unsupported backend fails like upstream.
pub fn resolve_google_function_calling_mode(
    tools: &[Tool],
    tool_choice: Option<&str>,
    supports_strict_mode: bool,
) -> Result<Option<GoogleFunctionCallingConfigMode>, String> {
    // Upstream computes useStrictMode first (the `.some()` may throw) and
    // short-circuits on the first strict tool.
    let mut use_strict_mode = false;
    for tool in tools {
        if resolve_json_schema_strict_sampling(tool, supports_strict_mode, None)? == Some(true) {
            use_strict_mode = true;
            break;
        }
    }
    if matches!(tool_choice, Some("none") | Some("any")) {
        return Ok(Some(map_tool_choice(tool_choice.expect("matched above"))));
    }
    if use_strict_mode {
        return Ok(Some(GoogleFunctionCallingConfigMode::Validated));
    }
    Ok(tool_choice.map(map_tool_choice))
}

// =============================================================================
// Stop reasons (google-shared.ts:439-483)
// =============================================================================

/// Upstream `FinishReason` (`@google/genai`): the candidate finish-reason
/// strings, serialized with their wire spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GoogleFinishReason {
    #[serde(rename = "FINISH_REASON_UNSPECIFIED")]
    FinishReasonUnspecified,
    #[serde(rename = "STOP")]
    Stop,
    #[serde(rename = "MAX_TOKENS")]
    MaxTokens,
    #[serde(rename = "SAFETY")]
    Safety,
    #[serde(rename = "RECITATION")]
    Recitation,
    #[serde(rename = "LANGUAGE")]
    Language,
    #[serde(rename = "OTHER")]
    Other,
    #[serde(rename = "BLOCKLIST")]
    Blocklist,
    #[serde(rename = "PROHIBITED_CONTENT")]
    ProhibitedContent,
    #[serde(rename = "SPII")]
    Spii,
    #[serde(rename = "MALFORMED_FUNCTION_CALL")]
    MalformedFunctionCall,
    #[serde(rename = "IMAGE_SAFETY")]
    ImageSafety,
    #[serde(rename = "UNEXPECTED_TOOL_CALL")]
    UnexpectedToolCall,
    #[serde(rename = "TOO_MANY_TOOL_CALLS")]
    TooManyToolCalls,
    #[serde(rename = "NO_IMAGE")]
    NoImage,
    #[serde(rename = "IMAGE_PROHIBITED_CONTENT")]
    ImageProhibitedContent,
    #[serde(rename = "IMAGE_RECITATION")]
    ImageRecitation,
    #[serde(rename = "IMAGE_OTHER")]
    ImageOther,
}

/// Upstream `mapStopReason` (google-shared.ts:441-469): STOP → stop,
/// MAX_TOKENS → length, every safety/other reason → error.
pub fn map_stop_reason(reason: GoogleFinishReason) -> StopReason {
    match reason {
        GoogleFinishReason::Stop => StopReason::Stop,
        GoogleFinishReason::MaxTokens => StopReason::Length,
        GoogleFinishReason::Blocklist
        | GoogleFinishReason::ProhibitedContent
        | GoogleFinishReason::Spii
        | GoogleFinishReason::Safety
        | GoogleFinishReason::ImageSafety
        | GoogleFinishReason::ImageProhibitedContent
        | GoogleFinishReason::ImageRecitation
        | GoogleFinishReason::ImageOther
        | GoogleFinishReason::Recitation
        | GoogleFinishReason::FinishReasonUnspecified
        | GoogleFinishReason::Other
        | GoogleFinishReason::Language
        | GoogleFinishReason::MalformedFunctionCall
        | GoogleFinishReason::UnexpectedToolCall
        | GoogleFinishReason::TooManyToolCalls
        | GoogleFinishReason::NoImage => StopReason::Error,
    }
}

/// Upstream `mapStopReasonString` (google-shared.ts:474-483): the raw-string
/// mapper for unparsed API responses.
pub fn map_stop_reason_string(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        _ => StopReason::Error,
    }
}

// =============================================================================
// Retry seam (google-shared.ts:485-515)
// =============================================================================

/// Upstream `retryGoogleRequest` (google-shared.ts:494-515): run the request
/// with the shared provider retry policy (408/409/429/5xx with backoff,
/// honoring retry-after). Upstream normalizes the SDK's headers-less ApiError
/// by attaching a missing `headers` property; the port's [`ProviderError`]
/// already carries `headers` as an `Option` and the shared policy treats a
/// missing header map exactly like the normalized upstream error, so this is
/// a straight delegation to [`retry_provider_request`]. The request signal
/// threads through: cancellation fails the request with the abort error and
/// aborts backoff sleeps.
pub async fn retry_google_request<T, F, Fut>(
    max_retries: u32,
    max_retry_delay_ms: Option<u64>,
    signal: Option<&CancellationToken>,
    request: F,
) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ProviderError>>,
{
    retry_provider_request(max_retries, max_retry_delay_ms, signal, request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::content::{ImageContent, TextContent, ThinkingContent, ToolCall};
    use crate::ai::types::message::{AssistantMessage, ToolResultMessage, UserMessage};
    use crate::ai::types::primitives::ThinkingLevelMap;
    use crate::ai::types::primitives::Usage;
    use crate::ai::types::tool::{ConstrainedSampling, JsonSchemaSampling, Strict};
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    const TS: i64 = 1758240000000;
    const VALID_SIG: &str = "AAAAAAAAAAAAAAAAAAAAAA==";

    // ---- fixtures ----

    fn google_model(id: &str, provider: &str, input: &[ModelInput]) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: id.to_string(),
            name: id.to_string(),
            api: "google-generative-ai".to_string(),
            provider: provider.to_string(),
            base_url: "https://example.com".to_string(),
            reasoning: true,
            thinking_level_map: None,
            input: input.to_vec(),
            cost: crate::ai::types::ModelCost::default(),
            context_window: 128000,
            max_tokens: 8192,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    fn vertex_model(id: &str, provider: &str) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            api: "google-vertex".to_string(),
            ..google_model(id, provider, &[ModelInput::Text])
        }
    }

    fn google_model_with_map(id: &str, provider: &str, map: ThinkingLevelMap) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            thinking_level_map: Some(map),
            ..google_model(id, provider, &[ModelInput::Text])
        }
    }

    fn level_map(entries: &[(&str, Option<&str>)]) -> ThinkingLevelMap {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.map(str::to_string)))
            .collect()
    }

    fn make_tool(parameters: Value) -> Tool {
        Tool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters,
            constrained_sampling: None,
        }
    }

    fn tool_call(id: &str, arguments: Value, thought_signature: Option<&str>) -> AssistantBlock {
        AssistantBlock::ToolCall(ToolCall {
            id: id.to_string(),
            name: "bash".to_string(),
            arguments,
            thought_signature: thought_signature.map(str::to_string),
            namespace: None,
        })
    }

    fn text_of(text: &str) -> AssistantBlock {
        AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    }

    fn thinking_of(thinking: &str, signature: Option<&str>) -> AssistantBlock {
        AssistantBlock::Thinking(ThinkingContent {
            thinking: thinking.to_string(),
            thinking_signature: signature.map(str::to_string),
            redacted: None,
        })
    }

    fn assistant_message(
        api: &str,
        provider: &str,
        model_id: &str,
        content: Vec<AssistantBlock>,
    ) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            api: api.to_string(),
            provider: provider.to_string(),
            model: model_id.to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: TS,
        })
    }

    fn user_message(content: &str) -> Message {
        Message::User(UserMessage {
            content: StringOrBlocks::Text(content.to_string()),
            timestamp: TS,
        })
    }

    fn tool_text_block(text: &str) -> TextOrImageBlock {
        TextOrImageBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    }

    fn tool_image_block() -> TextOrImageBlock {
        TextOrImageBlock::Image(ImageContent {
            data: "abc".to_string(),
            mime_type: "image/png".to_string(),
        })
    }

    fn tool_result(call_id: &str, tool_name: &str, content: Vec<TextOrImageBlock>) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            content,
            details: None,
            usage: None,
            is_error: false,
            timestamp: TS,
        })
    }

    fn context(messages: Vec<Message>) -> TranscriptContext {
        normalize_context(&Context {
            system_prompt: None,
            messages,
            tools: None,
        })
    }

    fn model_turn(contents: &[GoogleContent]) -> &GoogleContent {
        contents
            .iter()
            .find(|content| content.role == GoogleContentRole::Model)
            .expect("model turn present")
    }

    fn function_call_ids(contents: &[GoogleContent]) -> Vec<String> {
        contents
            .iter()
            .flat_map(|content| content.parts.iter())
            .filter_map(|part| part.function_call.as_ref())
            .filter_map(|call| call.id.clone())
            .collect()
    }

    fn function_response_ids(contents: &[GoogleContent]) -> Vec<String> {
        contents
            .iter()
            .flat_map(|content| content.parts.iter())
            .filter_map(|part| part.function_response.as_ref())
            .filter_map(|response| response.id.clone())
            .collect()
    }

    /// The gemini3-unsigned-tool-call oracle's `makeContext`: one assistant
    /// turn with two bash tool calls (first optionally signed) and two tool
    /// results.
    fn gemini3_context(
        api: &str,
        provider: &str,
        model_id: &str,
        thought_signature: Option<&str>,
    ) -> TranscriptContext {
        context(vec![
            user_message("Hi"),
            assistant_message(
                api,
                provider,
                model_id,
                vec![
                    tool_call("call_1", json!({"command": "echo hi"}), thought_signature),
                    tool_call("call_2", json!({"command": "ls -la"}), None),
                ],
            ),
            tool_result("call_1", "bash", vec![tool_text_block("hi")]),
            tool_result("call_2", "bash", vec![tool_text_block("files")]),
        ])
    }

    // ---- convertTools (google-shared-convert-tools.test.ts) ----

    #[test]
    fn strips_json_schema_meta_keys_when_use_parameters_true() {
        let tools = [make_tool(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$id": "urn:bash-tool",
            "$comment": "A bash tool for demonstration",
            "$defs": {"commandDef": {"type": "string"}},
            "definitions": {"legacyDef": {"type": "number"}},
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        }))];
        let result = convert_tools(&tools, true, true).unwrap().unwrap();
        let declaration = &result[0]["functionDeclarations"][0];
        assert_eq!(
            declaration["parameters"],
            json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
            })
        );
        for key in ["$schema", "$id", "$comment", "$defs", "definitions"] {
            assert!(declaration["parameters"].get(key).is_none(), "{key} leaked");
        }
    }

    #[test]
    fn recursively_strips_nested_json_schema_meta_keys() {
        let tools = [make_tool(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {
                "deep": {
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "$id": "urn:nested",
                    "type": "string",
                },
            },
        }))];
        let result = convert_tools(&tools, true, true).unwrap().unwrap();
        assert_eq!(
            result[0]["functionDeclarations"][0]["parameters"],
            json!({
                "type": "object",
                "properties": {"deep": {"type": "string"}},
            })
        );
    }

    #[test]
    fn preserves_ref_while_stripping_meta_keys() {
        let tools = [make_tool(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {
                "refProp": {"$ref": "#/$defs/someDef", "type": "string"},
            },
        }))];
        let result = convert_tools(&tools, true, true).unwrap().unwrap();
        assert_eq!(
            result[0]["functionDeclarations"][0]["parameters"],
            json!({
                "type": "object",
                "properties": {
                    "refProp": {"$ref": "#/$defs/someDef", "type": "string"},
                },
            })
        );
    }

    #[test]
    fn does_not_mutate_the_original_tool_parameters() {
        let parameters = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        });
        let original = parameters.clone();
        let tools = [make_tool(parameters)];
        convert_tools(&tools, true, true).unwrap();
        assert_eq!(tools[0].parameters, original);
    }

    #[test]
    fn preserves_schema_in_parameters_json_schema_when_use_parameters_false() {
        let tools = [make_tool(json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        }))];
        let result = convert_tools(&tools, false, true).unwrap().unwrap();
        assert_eq!(
            result[0]["functionDeclarations"][0]["parametersJsonSchema"],
            json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
            })
        );
    }

    #[test]
    fn handles_tools_without_schema_gracefully() {
        let tools = [make_tool(json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        }))];
        let result = convert_tools(&tools, true, true).unwrap().unwrap();
        assert_eq!(
            result[0]["functionDeclarations"][0]["parameters"],
            json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
            })
        );
    }

    #[test]
    fn uses_validated_function_calling_for_strict_tools_on_gemini_3() {
        let tool = Tool {
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict: Strict::Require,
            })),
            ..make_tool(json!({"type": "object", "properties": {}}))
        };
        assert!(supports_google_strict_tool_sampling(
            "gemini-3.1-pro-preview"
        ));
        assert!(!supports_google_strict_tool_sampling("gemini-2.5-pro"));
        assert_eq!(
            resolve_google_function_calling_mode(std::slice::from_ref(&tool), None, true).unwrap(),
            Some(GoogleFunctionCallingConfigMode::Validated)
        );
        let error = resolve_google_function_calling_mode(std::slice::from_ref(&tool), None, false)
            .unwrap_err();
        assert!(
            error.starts_with("Tool \"test_tool\" requires JSON-schema constrained sampling"),
            "{error}"
        );
    }

    #[test]
    fn returns_none_for_empty_tool_list() {
        assert_eq!(convert_tools(&[], false, true).unwrap(), None);
        assert_eq!(convert_tools(&[], true, true).unwrap(), None);
    }

    // ---- Gemini 3 unsigned tool calls (google-shared-gemini3-unsigned-tool-call.test.ts) ----

    #[test]
    fn preserves_tool_call_ids_for_gemini_3_history() {
        let models = [
            google_model("gemini-3-pro-preview", "google", &[ModelInput::Text]),
            google_model("gemini-3.6-flash", "google", &[ModelInput::Text]),
            vertex_model("gemini-3-pro-preview", "google-vertex"),
        ];
        for model in &models {
            let transcript = gemini3_context(&model.api, &model.provider, &model.id, None);
            let contents = convert_messages(model, &transcript);
            assert_eq!(function_call_ids(&contents), ["call_1", "call_2"]);
            assert_eq!(function_response_ids(&contents), ["call_1", "call_2"]);
        }
    }

    #[test]
    fn adds_no_skip_thought_signature_validator_for_unsigned_google_gen_ai_tool_calls() {
        let model = google_model("gemini-3-pro-preview", "google", &[ModelInput::Text]);
        let transcript = gemini3_context(&model.api, &model.provider, "other-model", None);
        let contents = convert_messages(&model, &transcript);
        let turn = model_turn(&contents);
        let function_call_parts: Vec<&GooglePart> = turn
            .parts
            .iter()
            .filter(|part| part.function_call.is_some())
            .collect();
        assert_eq!(function_call_parts.len(), 2);
        assert!(function_call_parts
            .iter()
            .all(|part| part.thought_signature.is_none()));
        let serialized = serde_json::to_string(turn).unwrap();
        assert!(!serialized.contains("skip_thought_signature_validator"));
        let historical_text = turn
            .parts
            .iter()
            .filter_map(|part| part.text.as_deref())
            .filter(|text| text.contains("Historical context"))
            .count();
        assert_eq!(historical_text, 0);
    }

    #[test]
    fn adds_no_skip_thought_signature_validator_for_unsigned_vertex_tool_calls() {
        let model = vertex_model("gemini-3-pro-preview", "google-vertex");
        let transcript = gemini3_context(&model.api, &model.provider, &model.id, None);
        let contents = convert_messages(&model, &transcript);
        let turn = model_turn(&contents);
        let function_call_parts: Vec<&GooglePart> = turn
            .parts
            .iter()
            .filter(|part| part.function_call.is_some())
            .collect();
        assert_eq!(function_call_parts.len(), 2);
        assert!(function_call_parts
            .iter()
            .all(|part| part.thought_signature.is_none()));
        let serialized = serde_json::to_string(turn).unwrap();
        assert!(!serialized.contains("skip_thought_signature_validator"));
    }

    #[test]
    fn preserves_valid_thought_signature_for_same_provider_and_model() {
        let model = google_model("gemini-3-pro-preview", "google", &[ModelInput::Text]);
        let transcript = gemini3_context(&model.api, &model.provider, &model.id, Some(VALID_SIG));
        let contents = convert_messages(&model, &transcript);
        let turn = model_turn(&contents);
        let function_call_parts: Vec<&GooglePart> = turn
            .parts
            .iter()
            .filter(|part| part.function_call.is_some())
            .collect();
        assert_eq!(function_call_parts.len(), 2);
        assert_eq!(
            function_call_parts[0].thought_signature.as_deref(),
            Some(VALID_SIG)
        );
        assert_eq!(function_call_parts[1].thought_signature, None);
    }

    #[test]
    fn adds_no_ids_or_signatures_for_non_gemini_3_models() {
        let model = google_model("gemini-2.5-flash", "google", &[ModelInput::Text]);
        let transcript = gemini3_context(&model.api, &model.provider, "other-model", None);
        let contents = convert_messages(&model, &transcript);
        let function_call_parts: Vec<&GooglePart> = model_turn(&contents)
            .parts
            .iter()
            .filter(|part| part.function_call.is_some())
            .collect();
        assert_eq!(function_call_parts.len(), 2);
        assert!(function_call_parts.iter().all(|part| part
            .function_call
            .as_ref()
            .unwrap()
            .id
            .is_none()));
        assert!(function_call_parts
            .iter()
            .all(|part| part.thought_signature.is_none()));
        let function_response_parts: Vec<&GooglePart> = contents
            .iter()
            .flat_map(|content| content.parts.iter())
            .filter(|part| part.function_response.is_some())
            .collect();
        assert_eq!(function_response_parts.len(), 2);
        assert!(function_response_parts.iter().all(|part| part
            .function_response
            .as_ref()
            .unwrap()
            .id
            .is_none()));
    }

    #[test]
    fn requires_tool_call_id_table() {
        assert!(!requires_tool_call_id("gemini-2.5-flash"));
        assert!(requires_tool_call_id("gemini-3.6-flash"));
        assert!(requires_tool_call_id("claude-sonnet-4-5"));
        assert!(requires_tool_call_id("gpt-oss-120b"));
    }

    #[test]
    fn model_id_matchers_pin_the_hand_rolled_regex_ports() {
        // /gemini-3(?:\.\d+)?-(?:pro|flash)/ — docstring examples plus
        // boundary cases the scanner must get right.
        for id in [
            "gemini-3-flash-preview",
            "gemini-3.1-pro-preview",
            "gemini-3.8-flash",
            "gemini-3.12-flash",
            "foo-gemini-3-flash",
            // Unanchored at the end: "-pro" alone satisfies the tail.
            "gemini-3-pro-max",
        ] {
            assert!(
                uses_google_thinking_level(&google_model(id, "google", &[ModelInput::Text])),
                "{id}"
            );
        }
        for id in [
            "gemini-2.5-flash",
            "gemini-3x-flash",
            "gemini-3.-flash",
            "gemini-3.1.5-flash",
            "gemini-30-flash",
        ] {
            assert!(
                !uses_google_thinking_level(&google_model(id, "google", &[ModelInput::Text])),
                "{id}"
            );
        }
        // Fixed-id forms and /gemma-?4/.
        for id in [
            "gemini-flash-latest",
            "gemini-flash-lite-latest",
            "gemma-4-small",
            "gemma4",
            "xgemma-4-y",
        ] {
            assert!(
                uses_google_thinking_level(&google_model(id, "google", &[ModelInput::Text])),
                "{id}"
            );
        }
        // ^gemini(?:-live)?-(\d+) anchoring.
        assert_eq!(get_gemini_major_version("gemini-2.5-flash"), Some(2));
        assert_eq!(get_gemini_major_version("GEMINI-Live-2.5-Flash"), Some(2));
        assert_eq!(get_gemini_major_version("gemini-3-pro"), Some(3));
        assert_eq!(get_gemini_major_version("gemini-live"), None);
        assert_eq!(get_gemini_major_version("geminix-3"), None);
        assert_eq!(get_gemini_major_version("gemini_x-3"), None);
    }

    // ---- image tool result routing (google-shared-image-tool-result-routing.test.ts) ----

    fn image_routing_context(model: &Model) -> TranscriptContext {
        context(vec![
            user_message("read the files"),
            assistant_message(
                &model.api,
                &model.provider,
                &model.id,
                vec![
                    tool_call("call_a", json!({"path": "a.txt"}), None),
                    tool_call("call_img", json!({"path": "image.png"}), None),
                    tool_call("call_b", json!({"path": "b.txt"}), None),
                ],
            ),
            tool_result("call_a", "read", vec![tool_text_block("alpha text")]),
            tool_result("call_img", "read", vec![tool_image_block()]),
            tool_result("call_b", "read", vec![tool_text_block("beta text")]),
        ])
    }

    #[test]
    fn keeps_separate_synthetic_image_turn_for_gemini_2x() {
        let model = google_model(
            "gemini-2.5-flash",
            "google",
            &[ModelInput::Text, ModelInput::Image],
        );
        let contents = convert_messages(&model, &image_routing_context(&model));
        assert_eq!(contents.len(), 5);
        assert!(contents[2]
            .parts
            .iter()
            .all(|part| part.function_response.is_some()));
        assert_eq!(
            contents[3].parts[0].text.as_deref(),
            Some("Tool result image:")
        );
        assert!(contents[3].parts[1].inline_data.is_some());
        assert!(contents[4].parts[0].function_response.is_some());
    }

    #[test]
    fn nests_image_tool_results_for_gemini_3() {
        let model = google_model(
            "gemini-3-pro-preview",
            "google",
            &[ModelInput::Text, ModelInput::Image],
        );
        let contents = convert_messages(&model, &image_routing_context(&model));
        assert_eq!(contents.len(), 3);
        let tool_result_turn = &contents[2];
        assert_eq!(tool_result_turn.parts.len(), 3);
        let image_response = tool_result_turn.parts[1]
            .function_response
            .as_ref()
            .expect("image function response");
        let parts = image_response.parts.as_ref().expect("nested parts");
        assert_eq!(parts.len(), 1);
        assert!(parts[0].inline_data.is_some());
    }

    // ---- signed empty blocks (google-shared-signed-empty-blocks.test.ts) ----

    #[test]
    fn keeps_a_signed_empty_thinking_block_so_its_signature_is_echoed_back() {
        let model = google_model("gemini-3-pro-preview", "google", &[ModelInput::Text]);
        let contents = convert_messages(
            &model,
            &context(vec![
                user_message("Hi"),
                assistant_message(
                    &model.api,
                    &model.provider,
                    &model.id,
                    vec![
                        thinking_of("", Some(VALID_SIG)),
                        tool_call("call_1", json!({"command": "ls"}), None),
                    ],
                ),
            ]),
        );
        let signed: Vec<&GooglePart> = model_turn(&contents)
            .parts
            .iter()
            .filter(|part| part.thought_signature.as_deref() == Some(VALID_SIG))
            .collect();
        assert_eq!(signed.len(), 1);
        assert_eq!(signed[0].thought, Some(true));
    }

    #[test]
    fn keeps_a_signed_empty_text_block_the_same_way() {
        let model = google_model("gemini-3-pro-preview", "google", &[ModelInput::Text]);
        let contents = convert_messages(
            &model,
            &context(vec![
                user_message("Hi"),
                assistant_message(
                    &model.api,
                    &model.provider,
                    &model.id,
                    vec![
                        AssistantBlock::Text(TextContent {
                            text: String::new(),
                            text_signature: Some(VALID_SIG.to_string()),
                        }),
                        tool_call("call_1", json!({"command": "ls"}), None),
                    ],
                ),
            ]),
        );
        let signed: Vec<&GooglePart> = model_turn(&contents)
            .parts
            .iter()
            .filter(|part| part.thought_signature.as_deref() == Some(VALID_SIG))
            .collect();
        assert_eq!(signed.len(), 1);
    }

    #[test]
    fn still_drops_unsigned_empty_blocks() {
        let model = google_model("gemini-3-pro-preview", "google", &[ModelInput::Text]);
        let contents = convert_messages(
            &model,
            &context(vec![
                user_message("Hi"),
                assistant_message(
                    &model.api,
                    &model.provider,
                    &model.id,
                    vec![
                        thinking_of("", None),
                        text_of("   "),
                        tool_call("call_1", json!({"command": "ls"}), None),
                    ],
                ),
            ]),
        );
        let turn = model_turn(&contents);
        assert_eq!(turn.parts.len(), 1);
        assert!(turn.parts[0].function_call.is_some());
    }

    #[test]
    fn still_drops_signed_empty_blocks_from_a_different_provider_or_model() {
        let model = google_model("gemini-3-pro-preview", "google", &[ModelInput::Text]);
        let contents = convert_messages(
            &model,
            &context(vec![
                user_message("Hi"),
                assistant_message(
                    &model.api,
                    &model.provider,
                    "other-model",
                    vec![
                        thinking_of("", Some(VALID_SIG)),
                        AssistantBlock::Text(TextContent {
                            text: String::new(),
                            text_signature: Some(VALID_SIG.to_string()),
                        }),
                        tool_call("call_1", json!({"command": "ls"}), None),
                    ],
                ),
            ]),
        );
        let turn = model_turn(&contents);
        assert_eq!(turn.parts.len(), 1);
        assert!(turn.parts[0].function_call.is_some());
        let serialized = serde_json::to_string(turn).unwrap();
        assert!(!serialized.contains(VALID_SIG));
    }

    // ---- thinking level map (google-thinking-level-map.test.ts, unit half) ----

    #[test]
    fn exhaustively_resolves_supported_logical_levels_and_mapping_values() {
        let model = google_model_with_map("gemini-3.7-flash", "test-google", level_map(&[]));
        for (level, expected) in [
            (ThinkingLevel::Minimal, ResolvedGoogleThinkingLevel::Minimal),
            (ThinkingLevel::Low, ResolvedGoogleThinkingLevel::Low),
            (ThinkingLevel::Medium, ResolvedGoogleThinkingLevel::Medium),
            (ThinkingLevel::High, ResolvedGoogleThinkingLevel::High),
        ] {
            assert_eq!(
                resolve_google_thinking_level(&model, level).unwrap(),
                expected
            );
        }

        let mapped = [
            ("minimal", ResolvedGoogleThinkingLevel::Minimal),
            ("low", ResolvedGoogleThinkingLevel::Low),
            ("medium", ResolvedGoogleThinkingLevel::Medium),
            ("high", ResolvedGoogleThinkingLevel::High),
            ("MINIMAL", ResolvedGoogleThinkingLevel::Minimal),
            ("LOW", ResolvedGoogleThinkingLevel::Low),
            ("MEDIUM", ResolvedGoogleThinkingLevel::Medium),
            ("HIGH", ResolvedGoogleThinkingLevel::High),
        ];
        for (value, expected) in mapped {
            let model = google_model_with_map(
                "gemini-3.7-flash",
                "test-google",
                level_map(&[
                    ("high", Some(value)),
                    ("xhigh", Some(value)),
                    ("max", Some(value)),
                ]),
            );
            assert_eq!(
                resolve_google_thinking_level(&model, ThinkingLevel::High).unwrap(),
                expected
            );
            assert_eq!(
                resolve_google_thinking_level(&model, ThinkingLevel::Xhigh).unwrap(),
                expected
            );
            assert_eq!(
                resolve_google_thinking_level(&model, ThinkingLevel::Max).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn rejects_unsupported_google_thinking_level_mappings() {
        let model = google_model_with_map(
            "gemini-3.7-flash",
            "test-google",
            level_map(&[("xhigh", Some("extreme"))]),
        );
        let error = resolve_google_thinking_level(&model, ThinkingLevel::Xhigh).unwrap_err();
        assert_eq!(
            error,
            "Unsupported Google thinking level mapping for test-google/gemini-3.7-flash: xhigh -> extreme"
        );
        let model = google_model_with_map("gemini-3.7-flash", "test-google", level_map(&[]));
        let error = resolve_google_thinking_level(&model, ThinkingLevel::Max).unwrap_err();
        assert_eq!(
            error,
            "Unsupported Google thinking level mapping for test-google/gemini-3.7-flash: max -> undefined"
        );
    }

    #[test]
    fn honors_uppercase_provider_values_for_standard_levels() {
        let model = google_model_with_map(
            "gemini-3.7-flash",
            "test-google",
            level_map(&[("high", Some("LOW"))]),
        );
        assert_eq!(
            resolve_google_thinking_level(&model, ThinkingLevel::High).unwrap(),
            ResolvedGoogleThinkingLevel::Low
        );
    }

    // ---- thinking disable (google-thinking-disable.test.ts, unit half) ----

    #[test]
    fn disabled_thinking_uses_budget_zero_for_budget_controlled_models() {
        let model = google_model("gemini-2.5-flash", "google", &[ModelInput::Text]);
        assert_eq!(
            get_disabled_google_thinking_config(&model).unwrap(),
            GoogleThinkingConfig {
                thinking_budget: Some(0),
                ..Default::default()
            }
        );
    }

    #[test]
    fn disabled_thinking_uses_budget_zero_when_off_is_supported() {
        // Default map: "off" is supported, so the clamp stays at "off".
        let model = google_model("gemini-3-flash-preview", "google", &[ModelInput::Text]);
        assert_eq!(
            get_disabled_google_thinking_config(&model).unwrap(),
            GoogleThinkingConfig {
                thinking_budget: Some(0),
                ..Default::default()
            }
        );
    }

    #[test]
    fn disabled_thinking_falls_back_to_the_lowest_supported_level() {
        // Oracle: "uses the lowest supported level when reasoning is omitted"
        // — the endpoint pins { thinkingLevel: "LOW" } from this function.
        let model = google_model_with_map(
            "gemini-3.8-flash",
            "google",
            level_map(&[
                ("off", None),
                ("minimal", None),
                ("low", Some("low")),
                ("medium", Some("medium")),
                ("high", Some("high")),
                ("xhigh", None),
                ("max", None),
            ]),
        );
        assert_eq!(
            get_disabled_google_thinking_config(&model).unwrap(),
            GoogleThinkingConfig {
                thinking_level: Some(GoogleApiThinkingLevel::Low),
                ..Default::default()
            }
        );
    }

    #[test]
    fn thinking_config_serializes_to_the_google_wire_form() {
        assert_eq!(
            serde_json::to_string(&GoogleThinkingConfig {
                thinking_budget: Some(0),
                ..Default::default()
            })
            .unwrap(),
            r#"{"thinkingBudget":0}"#
        );
        // Negative budgets are the dynamic-thinking signal (-1) the endpoints
        // pass through.
        assert_eq!(
            serde_json::to_string(&GoogleThinkingConfig {
                thinking_budget: Some(-1),
                ..Default::default()
            })
            .unwrap(),
            r#"{"thinkingBudget":-1}"#
        );
        assert_eq!(
            serde_json::to_string(&GoogleThinkingConfig {
                thinking_level: Some(GoogleApiThinkingLevel::Low),
                ..Default::default()
            })
            .unwrap(),
            r#"{"thinkingLevel":"LOW"}"#
        );
        assert_eq!(
            serde_json::to_string(&GoogleThinkingConfig {
                include_thoughts: Some(true),
                thinking_level: Some(GoogleApiThinkingLevel::Medium),
                ..Default::default()
            })
            .unwrap(),
            r#"{"includeThoughts":true,"thinkingLevel":"MEDIUM"}"#
        );
    }

    // ---- thinking signature (google-thinking-signature.test.ts) ----

    #[test]
    fn treats_thought_true_as_thinking() {
        assert!(is_thinking_part(&GooglePart {
            thought: Some(true),
            thought_signature: None,
            ..Default::default()
        }));
        assert!(is_thinking_part(&GooglePart {
            thought: Some(true),
            thought_signature: Some("opaque-signature".to_string()),
            ..Default::default()
        }));
    }

    #[test]
    fn does_not_treat_thought_signature_alone_as_thinking() {
        assert!(!is_thinking_part(&GooglePart {
            thought: None,
            thought_signature: Some("opaque-signature".to_string()),
            ..Default::default()
        }));
        assert!(!is_thinking_part(&GooglePart {
            thought: Some(false),
            thought_signature: Some("opaque-signature".to_string()),
            ..Default::default()
        }));
        assert!(!is_thinking_part(&GooglePart {
            thought: None,
            thought_signature: None,
            ..Default::default()
        }));
        assert!(!is_thinking_part(&GooglePart {
            thought: Some(false),
            thought_signature: Some(String::new()),
            ..Default::default()
        }));
    }

    #[test]
    fn retains_existing_signature_when_subsequent_deltas_omit_it() {
        let first = retain_thought_signature(None, Some("sig-1"));
        assert_eq!(first.as_deref(), Some("sig-1"));
        let second = retain_thought_signature(first.as_deref(), None);
        assert_eq!(second.as_deref(), Some("sig-1"));
        let third = retain_thought_signature(second.as_deref(), Some(""));
        assert_eq!(third.as_deref(), Some("sig-1"));
    }

    #[test]
    fn updates_signature_when_a_new_non_empty_signature_arrives() {
        assert_eq!(
            retain_thought_signature(Some("sig-1"), Some("sig-2")).as_deref(),
            Some("sig-2")
        );
    }

    // ---- stop reasons ----

    #[test]
    fn maps_finish_reasons() {
        assert_eq!(map_stop_reason(GoogleFinishReason::Stop), StopReason::Stop);
        assert_eq!(
            map_stop_reason(GoogleFinishReason::MaxTokens),
            StopReason::Length
        );
        for reason in [
            GoogleFinishReason::Blocklist,
            GoogleFinishReason::ProhibitedContent,
            GoogleFinishReason::Spii,
            GoogleFinishReason::Safety,
            GoogleFinishReason::ImageSafety,
            GoogleFinishReason::ImageProhibitedContent,
            GoogleFinishReason::ImageRecitation,
            GoogleFinishReason::ImageOther,
            GoogleFinishReason::Recitation,
            GoogleFinishReason::FinishReasonUnspecified,
            GoogleFinishReason::Other,
            GoogleFinishReason::Language,
            GoogleFinishReason::MalformedFunctionCall,
            GoogleFinishReason::UnexpectedToolCall,
            GoogleFinishReason::TooManyToolCalls,
            GoogleFinishReason::NoImage,
        ] {
            assert_eq!(map_stop_reason(reason), StopReason::Error, "{reason:?}");
        }
    }

    #[test]
    fn maps_raw_stop_reason_strings() {
        assert_eq!(map_stop_reason_string("STOP"), StopReason::Stop);
        assert_eq!(map_stop_reason_string("MAX_TOKENS"), StopReason::Length);
        assert_eq!(map_stop_reason_string("SAFETY"), StopReason::Error);
        assert_eq!(map_stop_reason_string("SOMETHING_NEW"), StopReason::Error);
    }

    // ---- function calling modes ----

    #[test]
    fn maps_tool_choices() {
        assert_eq!(
            map_tool_choice("auto"),
            GoogleFunctionCallingConfigMode::Auto
        );
        assert_eq!(
            map_tool_choice("none"),
            GoogleFunctionCallingConfigMode::None
        );
        assert_eq!(map_tool_choice("any"), GoogleFunctionCallingConfigMode::Any);
        assert_eq!(
            map_tool_choice("whatever"),
            GoogleFunctionCallingConfigMode::Auto
        );
    }

    #[test]
    fn resolves_function_calling_modes() {
        let plain = [make_tool(json!({"type": "object", "properties": {}}))];
        let strict = [Tool {
            constrained_sampling: Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict: Strict::Require,
            })),
            ..make_tool(json!({"type": "object", "properties": {}}))
        }];
        // Explicit none/any win before strict handling.
        assert_eq!(
            resolve_google_function_calling_mode(&strict, Some("none"), true).unwrap(),
            Some(GoogleFunctionCallingConfigMode::None)
        );
        assert_eq!(
            resolve_google_function_calling_mode(&strict, Some("any"), true).unwrap(),
            Some(GoogleFunctionCallingConfigMode::Any)
        );
        // Strict tools force VALIDATED.
        assert_eq!(
            resolve_google_function_calling_mode(&strict, Some("auto"), true).unwrap(),
            Some(GoogleFunctionCallingConfigMode::Validated)
        );
        assert_eq!(
            resolve_google_function_calling_mode(&strict, None, true).unwrap(),
            Some(GoogleFunctionCallingConfigMode::Validated)
        );
        // Plain tools follow the choice, or nothing.
        assert_eq!(
            resolve_google_function_calling_mode(&plain, Some("auto"), true).unwrap(),
            Some(GoogleFunctionCallingConfigMode::Auto)
        );
        assert_eq!(
            resolve_google_function_calling_mode(&plain, None, true).unwrap(),
            None
        );
    }

    // ---- retry (google-shared-retry.test.ts) ----

    fn google_api_error(status: u16) -> ProviderError {
        // Shaped like @google/genai's ApiError: a status but no headers.
        ProviderError {
            status: Some(status),
            headers: None,
            message: format!("got status: {status}"),
        }
    }

    #[tokio::test]
    async fn retries_a_headers_less_sdk_error_with_a_retryable_status() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_closure = attempts.clone();
        let result = retry_google_request(1, None, None, move || {
            let attempts = attempts_closure.clone();
            async move {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(google_api_error(429))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_when_max_retries_is_unset() {
        let error = google_api_error(429);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_closure = attempts.clone();
        let result: Result<&str, ProviderError> = retry_google_request(0, None, None, move || {
            let error = error.clone();
            let attempts = attempts_closure.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(error)
            }
        })
        .await;
        assert_eq!(result.unwrap_err().message, "got status: 429");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn does_not_retry_a_non_retryable_status() {
        let error = google_api_error(400);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_closure = attempts.clone();
        let result: Result<&str, ProviderError> = retry_google_request(2, None, None, move || {
            let error = error.clone();
            let attempts = attempts_closure.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(error)
            }
        })
        .await;
        assert_eq!(result.unwrap_err().message, "got status: 400");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
