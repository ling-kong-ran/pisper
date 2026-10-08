//! OpenAI Responses shared protocol — full port of upstream
//! `packages/ai/src/api/openai-responses-shared.ts` (793 lines): the
//! transcript → Responses-input conversion, the Responses tool conversion,
//! and the SSE event → [`AssistantMessageEvent`] processor shared by the
//! three Responses endpoints (`openai-responses.ts`, `azure-openai-responses.ts`,
//! `openai-codex-responses.ts`).
//!
//! - `convertResponsesMessages` (lines 143-353): leading system message →
//!   `developer`/`system` instruction item, mid-conversation system updates,
//!   `additional_tools`/`tool_search_call`/`tool_search_output` items, user
//!   `input_text`/`input_image` items, assistant `message` items with id
//!   preservation (`TextSignatureV1` legacy + v1 JSON, `msg_pi_N` fallbacks,
//!   64-char clamp), thinking replay as raw `reasoning` items (the stored
//!   `thinkingSignature` JSON is pushed verbatim, encrypted content and all),
//!   `function_call`/`custom_tool_call` replay with foreign tool-call-id
//!   normalization (`fc_<shortHash>` for ids that did not come from this
//!   provider/api, `call_id|item_id` pipe pairs), and
//!   `function_call_output`/`custom_tool_call_output` results (text-only, or
//!   `input_text`+`input_image` arrays for image-capable models).
//! - `convertResponsesTools` (lines 359-396): grammar tools → `custom` tools
//!   with a `grammar` format, JSON-schema tools with the upstream
//!   `strict`/`supportsStrictMode` tri-state (`strict: null` supported), and
//!   `defer_loading` for tool-search results.
//! - `processResponsesStream` (lines 402-793) as [`ResponsesStreamProcessor`]:
//!   output-item slots (thinking/text/toolCall), text/refusal/reasoning
//!   summary+text deltas, function-call argument deltas with live partial-JSON
//!   parsing, custom-tool input through the grammar buffer, authoritative
//!   `output_item.done` assembly (message ids + text signatures, reasoning
//!   signatures, partial-JSON scratch cleanup), terminal
//!   `response.completed`/`response.incomplete`/`response.failed` handling
//!   with usage math (`input_tokens_details.cached_tokens` /
//!   `cache_write_tokens`, `output_tokens_details.reasoning_tokens`),
//!   `calculateCost`, the Azure encrypted-content backfill (pi issue #6409),
//!   and the no-terminal-event guard.
//! - Session-affinity headers for Responses endpoints (upstream
//!   `openai-responses.ts` `detectSessionAffinityFormat` lines 50-52 and the
//!   `createClient` block lines 258-267): `openrouter` → `x-session-id`,
//!   `openai` → `session_id` + `x-client-request-id`, `openai-nosession` →
//!   `x-client-request-id` only.
//!
//! T8 seam: [`ResponsesStreamProcessor`] consumes typed
//! [`ResponsesStreamEvent`] payloads — T8 parses SSE frames
//! (`serde_json::from_str::<ResponsesStreamEvent>` via the infallible
//! [`ResponsesStreamEvent::from_value`], unknown types →
//! [`ResponsesStreamEvent::Unhandled`]), emits `start`, drives
//! [`ResponsesStreamProcessor::process_event`], then
//! [`ResponsesStreamProcessor::finish`], and maps `Err` onto the `error`
//! event exactly like the upstream endpoint catch blocks.
//!
//! Deviations from upstream, all structural:
//! - `sanitizeSurrogates` is a no-op: Rust `String` is UTF-8 and cannot hold
//!   unpaired surrogates.
//! - Upstream events carry the live `partial`; the port emits events without
//!   it and consumers reconstruct via `PartialAssistant` (the M2a contract).
//!   The processor's live state is readable through
//!   [`ResponsesStreamProcessor::output`].
//! - JSON object key order follows `serde_json` (sorted), not JS insertion
//!   order — same documented deviation as the other request builders. Visible
//!   only in the serialized `TextSignatureV1` (key-sorted) and item bodies.
//! - A `function_call`/`custom_tool_call` output item without an `id` is
//!   treated as unhandled; upstream would build a `call_x|undefined`
//!   composite id, which no real stream produces.
//! - `usage.reasoning` is `Some(...)` always (upstream assigns `|| 0`), and
//!   `cacheWrite1h` stays unset (Responses never reports it).

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use crate::ai::api::openai_completions::request::{
    get_grammar_tool_input, make_strict_json_schema, render_system_message_update,
    resolve_grammar_constrained_sampling, resolve_json_schema_strict_sampling, short_hash,
    transform_messages,
};
use crate::ai::api::openai_completions::stream::{
    append_grammar_tool_input_json_delta, parse_streaming_json, GrammarBuf,
};
use crate::ai::cost::calculate_cost;
use crate::ai::now_ms;
use crate::ai::transcript::{
    get_system_message_text, resolve_transcript, resolve_transcript_tools, TranscriptContext,
};
use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, TextOrImageBlock,
};
use crate::ai::types::primitives::{SessionAffinityFormat, StopReason, Usage, UsageCost};
use crate::ai::types::tool::Tool;
use crate::ai::types::{Model, ModelInput};

// =============================================================================
// Utilities (openai-responses-shared.ts:52-107)
// =============================================================================

/// JS string length in UTF-16 code units.
fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// JS `value.slice(0, max)` in UTF-16 code units. A cut surrogate pair
/// degrades to U+FFFD; real item ids are ASCII.
fn utf16_slice(value: &str, max: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(max).collect::<Vec<_>>())
}

/// Upstream `normalizeIdPart` (lines 152-156): replace every
/// non-`[a-zA-Z0-9_-]` UTF-16 code unit with `_`, clamp to 64 UTF-16 code
/// units (the OpenAI Responses id limit), then strip trailing underscores.
fn normalize_id_part(part: &str) -> String {
    let mut sanitized = String::with_capacity(part.len());
    for unit in part.encode_utf16() {
        let ch = match unit {
            0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A | 0x5F | 0x2D => {
                char::from_u32(u32::from(unit)).unwrap_or('_')
            }
            _ => '_',
        };
        sanitized.push(ch);
    }
    let clamped = utf16_slice(&sanitized, 64);
    clamped.trim_end_matches('_').to_string()
}

/// Upstream `buildForeignResponsesItemId` (lines 158-161): `fc_<shortHash>`
/// with the 64-char clamp.
fn build_foreign_responses_item_id(item_id: &str) -> String {
    let normalized = format!("fc_{}", short_hash(item_id));
    if utf16_len(&normalized) > 64 {
        utf16_slice(&normalized, 64)
    } else {
        normalized
    }
}

/// Upstream `encodeTextSignatureV1` (lines 52-56):
/// `JSON.stringify({v: 1, id, phase?})`.
fn encode_text_signature_v1(id: &str, phase: Option<&str>) -> String {
    let mut payload = Map::new();
    payload.insert("v".into(), json!(1));
    payload.insert("id".into(), json!(id));
    if let Some(phase) = phase {
        payload.insert("phase".into(), json!(phase));
    }
    Value::Object(payload).to_string()
}

/// Upstream `parseTextSignature` (lines 58-76): the `TextSignatureV1` JSON
/// form (`{v: 1, id, phase?}` with phase restricted to commentary/final_answer)
/// or the legacy plain-id string.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedTextSignature {
    id: String,
    phase: Option<String>,
}

fn parse_text_signature(signature: Option<&str>) -> Option<ParsedTextSignature> {
    let signature = signature?;
    if signature.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<Value>(signature) {
            let id = parsed.get("id").and_then(Value::as_str);
            if parsed.get("v").and_then(Value::as_f64) == Some(1.0) {
                if let Some(id) = id {
                    let phase = match parsed.get("phase").and_then(Value::as_str) {
                        Some("commentary") | Some("final_answer") => Some(
                            parsed
                                .get("phase")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        ),
                        _ => None,
                    };
                    return Some(ParsedTextSignature {
                        id: id.to_string(),
                        phase,
                    });
                }
            }
        }
        // Invalid JSON or not a v1 payload falls through to legacy handling.
    }
    Some(ParsedTextSignature {
        id: signature.to_string(),
        phase: None,
    })
}

/// Upstream `convertToolResultOutput` (lines 80-107): text-only results (or
/// image results on non-image models) collapse into one string; image-capable
/// models get an `input_text`/`input_image` array.
fn convert_tool_result_output(model: &Model, content: &[TextOrImageBlock]) -> Value {
    let text_result = content
        .iter()
        .filter_map(|block| match block {
            TextOrImageBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let images = content
        .iter()
        .filter_map(|block| match block {
            TextOrImageBlock::Image(image) => Some(image),
            _ => None,
        })
        .collect::<Vec<_>>();
    let has_text = !text_result.is_empty();

    if images.is_empty() || !model.input.contains(&ModelInput::Image) {
        return Value::String(if has_text {
            text_result
        } else if !images.is_empty() {
            "(see attached image)".to_string()
        } else {
            "(no tool output)".to_string()
        });
    }

    let mut output = Vec::new();
    if has_text {
        output.push(json!({"type": "input_text", "text": text_result}));
    }
    for image in images {
        output.push(json!({
            "type": "input_image",
            "detail": "auto",
            "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
        }));
    }
    Value::Array(output)
}

// =============================================================================
// Options (openai-responses-shared.ts:109-137)
// =============================================================================

/// Upstream `ConvertResponsesToolsOptions` (lines 132-137).
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertResponsesToolsOptions {
    /// Upstream `strict?: boolean | null` — the JS tri-state: `None` is
    /// `undefined` (defaults to `false`), `Some(None)` is `null` (serialized
    /// as JSON `null`, used by the codex endpoint to drop the default), and
    /// `Some(Some(strict))` is a boolean.
    pub strict: Option<Option<bool>>,
    /// Upstream `supportsStrictMode ?? true`.
    pub supports_strict_mode: bool,
    /// Upstream `supportsOpenAIGrammarTools ?? false`.
    pub supports_openai_grammar_tools: bool,
    /// Upstream `toolSearchResult`: add `defer_loading` to converted tools.
    pub tool_search_result: bool,
}

impl Default for ConvertResponsesToolsOptions {
    fn default() -> Self {
        ConvertResponsesToolsOptions {
            strict: None,
            supports_strict_mode: true,
            supports_openai_grammar_tools: false,
            tool_search_result: false,
        }
    }
}

/// Upstream `ConvertResponsesMessagesOptions` (lines 122-130).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConvertResponsesMessagesOptions {
    /// Upstream `includeSystemPrompt ?? true`: emit the leading system
    /// message as an instruction item.
    pub include_system_prompt: Option<bool>,
    /// Upstream `grammarToolInputProperties`: tool name → custom input
    /// property for grammar-constrained tools.
    pub grammar_tool_input_properties: HashMap<String, String>,
    /// Whether later system messages are sent in place; otherwise they are
    /// folded into the leading prompt (upstream
    /// `supportsMidConvoSystemMessages`).
    pub supports_mid_convo_system_messages: bool,
    /// Upstream `supportsAdditionalTools`: `additional_tools` items.
    pub supports_additional_tools: bool,
    /// Upstream `supportsToolSearch`: client-executed tool search items.
    pub supports_tool_search: bool,
    /// Upstream `toolOptions` forwarded to [`convert_responses_tools`].
    pub tool_options: Option<ConvertResponsesToolsOptions>,
}

/// Upstream `resolveServiceTier` hook (openai-responses-shared.ts:112-115):
/// chooses the tier the pricing hook sees from the response and request
/// tiers.
pub type ResolveServiceTierHook =
    Box<dyn FnMut(Option<&str>, Option<&str>) -> Option<String> + Send + Sync>;

/// Upstream `applyServiceTierPricing` hook (openai-responses-shared.ts:116-119):
/// adjusts the finalized usage cost for the effective service tier.
pub type ApplyServiceTierPricingHook = Box<dyn FnMut(&mut Usage, Option<String>) + Send + Sync>;

/// Upstream `OpenAIResponsesStreamOptions` (lines 109-120): the endpoint
/// supplied hooks the stream processor applies at finalize time.
#[derive(Default)]
pub struct ResponsesStreamOptions {
    /// Upstream `onProviderStreamEvent` (the openai-responses-shared.ts
    /// delta): observer for each parsed provider stream event before Pi
    /// normalization. Adapter-owned data, read-only; absent observer = no-op.
    pub callbacks: crate::ai::types::request_callbacks::RequestCallbacks,
    /// Upstream `serviceTier`: the request option echo used for pricing
    /// resolution when the response does not report its tier.
    pub service_tier: Option<String>,
    /// Upstream `grammarToolInputProperties`: tool name → custom input
    /// property for grammar/custom tool calls.
    pub grammar_tool_input_properties: HashMap<String, String>,
    /// Upstream `resolveServiceTier`: optional override choosing the tier the
    /// pricing hook sees (response tier, then request tier by default).
    pub resolve_service_tier: Option<ResolveServiceTierHook>,
    /// Upstream `applyServiceTierPricing`: optional pricing adjustment
    /// (flex/priority multipliers live in the endpoint, not here).
    pub apply_service_tier_pricing: Option<ApplyServiceTierPricingHook>,
}

// =============================================================================
// Message conversion (openai-responses-shared.ts:139-353)
// =============================================================================

/// Upstream `convertResponsesMessages` (lines 143-353): convert a normalized
/// transcript into the Responses `input` item list. Pure; errors mirror
/// upstream throws (invalid thinking-signature JSON, grammar tool input that
/// is not a string, unsupported strict schemas, missing grammar variants).
pub fn convert_responses_messages(
    model: &Model,
    context: &TranscriptContext,
    allowed_tool_call_providers: &HashSet<String>,
    options: &ConvertResponsesMessagesOptions,
) -> Result<Vec<Value>, String> {
    let normalized = resolve_transcript(
        context.clone(),
        Some(options.supports_mid_convo_system_messages),
    );
    let mut messages: Vec<Value> = Vec::new();

    // Upstream `normalizeToolCallId` (lines 163-175): per-provider tool-call
    // id normalization for the Responses wire, with foreign item ids hashed
    // into the `fc_<shortHash>` shape OpenAI requires.
    let normalize_tool_call_id = |id: &str, source: &AssistantMessage| -> String {
        if !allowed_tool_call_providers.contains(&model.provider) {
            return normalize_id_part(id);
        }
        if !id.contains('|') {
            return normalize_id_part(id);
        }
        // Upstream destructures the first two `split("|")` segments; further
        // segments are ignored.
        let mut segments = id.split('|');
        let call_id = segments.next().unwrap_or("");
        let item_id = segments.next().unwrap_or("");
        let normalized_call_id = normalize_id_part(call_id);
        let is_foreign_tool_call = source.provider != model.provider || source.api != model.api;
        let mut normalized_item_id = if is_foreign_tool_call {
            build_foreign_responses_item_id(item_id)
        } else {
            normalize_id_part(item_id)
        };
        // OpenAI Responses API requires item ids to start with "fc".
        if !normalized_item_id.starts_with("fc_") {
            normalized_item_id = normalize_id_part(&format!("fc_{normalized_item_id}"));
        }
        format!("{normalized_call_id}|{normalized_item_id}")
    };

    let transformed = transform_messages(model, normalized.messages(), &normalize_tool_call_id);
    let transcript_tools = resolve_transcript_tools(
        normalized.messages(),
        options.supports_additional_tools || options.supports_tool_search,
    );
    let include_initial_system_message = options.include_system_prompt.unwrap_or(true);
    // Upstream casts compat to `{ supportsDeveloperRole?: boolean }` and
    // checks `!== false`.
    let supports_developer_role = model
        .compat
        .as_ref()
        .and_then(|compat| compat.get("supportsDeveloperRole"))
        .and_then(Value::as_bool);
    let instruction_role = if model.reasoning && supports_developer_role != Some(false) {
        "developer"
    } else {
        "system"
    };

    let mut msg_index = 0usize;
    for (index, msg) in transformed.into_iter().enumerate() {
        // Upstream evaluates `sourceIndex++ === 0 && msg.role === "system"`:
        // the counter advances for every message regardless.
        let is_leading_system_message = index == 0 && matches!(msg, Message::System(_));
        // `continue` on empty user/assistant content skips the trailing
        // msgIndex++ upstream.
        let mut advance_index = !is_leading_system_message;
        match &msg {
            Message::System(system) => {
                if !is_leading_system_message {
                    // Upstream `appendSystemToolAdditions` (lines 182-210).
                    let tools = if transcript_tools.anchors_additions {
                        system.tools_added.clone().unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    if !tools.is_empty() {
                        if options.supports_additional_tools {
                            let converted = convert_responses_tools(
                                &tools,
                                options
                                    .tool_options
                                    .as_ref()
                                    .unwrap_or(&ConvertResponsesToolsOptions::default()),
                            )?;
                            messages.push(json!({
                                "type": "additional_tools",
                                "role": "developer",
                                "tools": converted
                            }));
                        } else if options.supports_tool_search {
                            let names: Vec<&str> =
                                tools.iter().map(|tool| tool.name.as_str()).collect();
                            let seed = format!("system:{msg_index}");
                            let call_id = format!(
                                "pi_tool_load_{}",
                                short_hash(&format!("{seed}:{}", names.join(",")))
                            );
                            messages.push(json!({
                                "type": "tool_search_call",
                                "call_id": call_id,
                                "execution": "client",
                                "status": "completed",
                                "arguments": {"query": names.join(" "), "limit": names.len()}
                            }));
                            let mut search_options =
                                options.tool_options.clone().unwrap_or_default();
                            search_options.tool_search_result = true;
                            let converted = convert_responses_tools(&tools, &search_options)?;
                            messages.push(json!({
                                "type": "tool_search_output",
                                "call_id": call_id,
                                "execution": "client",
                                "status": "completed",
                                "tools": converted
                            }));
                        }
                    }
                }
                if !is_leading_system_message || include_initial_system_message {
                    let text = if is_leading_system_message {
                        get_system_message_text(system)
                    } else {
                        render_system_message_update(system)
                    };
                    if !text.is_empty() {
                        messages.push(json!({"role": instruction_role, "content": text}));
                    }
                }
            }
            Message::User(user_msg) => match &user_msg.content {
                StringOrBlocks::Text(text) => {
                    messages.push(json!({
                        "role": "user",
                        "content": [{"type": "input_text", "text": text}]
                    }));
                }
                StringOrBlocks::Blocks(blocks) => {
                    let content: Vec<Value> = blocks
                        .iter()
                        .map(|item| match item {
                            TextOrImageBlock::Text(text) => {
                                json!({"type": "input_text", "text": text.text})
                            }
                            TextOrImageBlock::Image(image) => json!({
                                "type": "input_image",
                                "detail": "auto",
                                "image_url": format!("data:{};base64,{}", image.mime_type, image.data)
                            }),
                        })
                        .collect();
                    if content.is_empty() {
                        advance_index = false;
                    } else {
                        messages.push(json!({"role": "user", "content": content}));
                    }
                }
            },
            Message::Assistant(assistant) => {
                let output = convert_assistant_items(model, assistant, options, msg_index)?;
                if output.is_empty() {
                    advance_index = false;
                } else {
                    messages.extend(output);
                }
            }
            Message::ToolResult(result) => {
                let call_id = result
                    .tool_call_id
                    .split('|')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let output = convert_tool_result_output(model, &result.content);
                if options
                    .grammar_tool_input_properties
                    .contains_key(&result.tool_name)
                {
                    messages.push(json!({
                        "type": "custom_tool_call_output",
                        "call_id": call_id,
                        "output": output
                    }));
                } else {
                    messages.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output
                    }));
                }
            }
        }
        if advance_index {
            msg_index += 1;
        }
    }

    Ok(messages)
}

/// Upstream assistant-message conversion (lines 253-330): signed thinking →
/// raw reasoning items, text → `message` items with id preservation, tool
/// calls → `function_call`/`custom_tool_call` items with the cross-model id
/// drop rules and namespace handling.
fn convert_assistant_items(
    model: &Model,
    assistant: &AssistantMessage,
    options: &ConvertResponsesMessagesOptions,
    msg_index: usize,
) -> Result<Vec<Value>, String> {
    let is_same_provider_and_api =
        assistant.provider == model.provider && assistant.api == model.api;
    let is_same_model = is_same_provider_and_api && assistant.model == model.id;
    let is_different_model = is_same_provider_and_api && assistant.model != model.id;
    let mut output: Vec<Value> = Vec::new();
    let mut text_block_index = 0usize;

    for block in &assistant.content {
        match block {
            AssistantBlock::Thinking(thinking) => {
                // Only signed thinking replays (the raw reasoning item JSON,
                // encrypted content included); unsigned thinking is dropped.
                if let Some(signature) = &thinking.thinking_signature {
                    let reasoning_item: Value = serde_json::from_str(signature)
                        .map_err(|error| format!("Invalid thinking signature JSON: {error}"))?;
                    output.push(reasoning_item);
                }
            }
            AssistantBlock::Text(text) => {
                let parsed = parse_text_signature(text.text_signature.as_deref());
                let fallback_message_id = if text_block_index == 0 {
                    format!("msg_pi_{msg_index}")
                } else {
                    format!("msg_pi_{msg_index}_{text_block_index}")
                };
                text_block_index += 1;
                // OpenAI requires ids to be at most 64 characters.
                let msg_id = match &parsed {
                    Some(parsed) if utf16_len(&parsed.id) > 64 => {
                        format!("msg_{}", short_hash(&parsed.id))
                    }
                    Some(parsed) => parsed.id.clone(),
                    None => fallback_message_id,
                };
                let mut item = json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": text.text, "annotations": []}],
                    "status": "completed",
                    "id": msg_id
                });
                if let Some(phase) = parsed.as_ref().and_then(|parsed| parsed.phase.as_deref()) {
                    item["phase"] = json!(phase);
                }
                output.push(item);
            }
            AssistantBlock::ToolCall(call) => {
                let mut segments = call.id.split('|');
                let call_id = segments.next().unwrap_or("").to_string();
                let mut item_id: Option<String> = segments.next().map(str::to_string);
                let custom_input_property = options.grammar_tool_input_properties.get(&call.name);
                // For different-model messages drop item ids to avoid OpenAI's
                // reasoning pairing validation (it tracks which item ids were
                // paired with rs_* reasoning items). Also drop ids that do not
                // match the replayed item type (v1.0.0): function_call ids must
                // be fc_* and custom_tool_call ids must be ctc_*. Foreign tool
                // call ids are normalized to fc_*, and a call can switch
                // between the two types when grammar tool support differs.
                let item_id_prefix = if custom_input_property.is_none() {
                    "fc_"
                } else {
                    "ctc_"
                };
                let has_expected_prefix = item_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with(item_id_prefix));
                if is_different_model || !has_expected_prefix {
                    item_id = None;
                }
                let namespace = if is_same_model {
                    call.namespace.clone()
                } else {
                    None
                };
                match custom_input_property {
                    Some(property) => {
                        let input = get_grammar_tool_input(&call.name, &call.arguments, property)?;
                        let mut item = json!({
                            "type": "custom_tool_call",
                            "call_id": call_id,
                            "name": call.name,
                            "input": input
                        });
                        if let Some(item_id) = &item_id {
                            item["id"] = json!(item_id);
                        }
                        if let Some(namespace) = &namespace {
                            item["namespace"] = json!(namespace);
                        }
                        output.push(item);
                    }
                    None => {
                        let mut item = json!({
                            "type": "function_call",
                            "call_id": call_id,
                            "name": call.name,
                            "arguments": serde_json::to_string(&call.arguments)
                                .unwrap_or_default()
                        });
                        if let Some(item_id) = &item_id {
                            item["id"] = json!(item_id);
                        }
                        if let Some(namespace) = &namespace {
                            item["namespace"] = json!(namespace);
                        }
                        output.push(item);
                    }
                }
            }
        }
    }
    Ok(output)
}

// =============================================================================
// Tool conversion (openai-responses-shared.ts:355-396)
// =============================================================================

/// Upstream `convertResponsesTools` (lines 359-396): grammar tools become
/// `custom` tools with a grammar format; everything else becomes a `function`
/// tool with the strict tri-state resolved per tool.
pub fn convert_responses_tools(
    tools: &[Tool],
    options: &ConvertResponsesToolsOptions,
) -> Result<Vec<Value>, String> {
    // Upstream line 360: `options?.strict === undefined ? false : options.strict`
    // — `null` survives as the default, `undefined` becomes `false`.
    let default_strict: Option<bool> = match options.strict {
        None => Some(false),
        Some(strict) => strict,
    };
    tools
        .iter()
        .map(|tool| {
            let grammar =
                resolve_grammar_constrained_sampling(tool, options.supports_openai_grammar_tools)?;
            if let Some(grammar) = grammar {
                let mut custom = Map::new();
                custom.insert("type".into(), json!("custom"));
                custom.insert("name".into(), json!(tool.name));
                custom.insert("description".into(), json!(tool.description));
                custom.insert(
                    "format".into(),
                    json!({"type": "grammar", "syntax": grammar.format, "definition": grammar.definition}),
                );
                if options.tool_search_result {
                    custom.insert("defer_loading".into(), json!(true));
                }
                return Ok(Value::Object(custom));
            }

            let constrained =
                resolve_json_schema_strict_sampling(tool, options.supports_strict_mode, None)?;
            // Upstream line 381: `constrainedStrict ?? defaultStrict`.
            let strict: Option<bool> = match constrained {
                Some(strict) => Some(strict),
                None => default_strict,
            };
            let parameters = if strict == Some(true) {
                make_strict_json_schema(&tool.parameters, None)?
            } else {
                tool.parameters.clone()
            };
            let mut function = Map::new();
            function.insert("type".into(), json!("function"));
            function.insert("name".into(), json!(tool.name));
            function.insert("description".into(), json!(tool.description));
            function.insert("parameters".into(), parameters);
            if options.tool_search_result {
                function.insert("defer_loading".into(), json!(true));
            }
            if options.supports_strict_mode {
                function.insert(
                    "strict".into(),
                    match strict {
                        Some(strict) => json!(strict),
                        None => Value::Null,
                    },
                );
            }
            Ok(Value::Object(function))
        })
        .collect()
}

// =============================================================================
// Typed SSE payloads (the T8 seam)
// =============================================================================

fn opt_str(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn opt_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

/// Upstream `ResponseError` (`error.code`/`error.message`, both optional).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponseError {
    pub code: Option<String>,
    pub message: Option<String>,
}

/// Upstream `response.incomplete_details` with the open `reason` field
/// (upstream checks `typeof reason === "string"`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IncompleteDetails {
    pub reason: Option<Value>,
}

/// Upstream `response.usage` with its two detail objects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponsesUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub input_tokens_details: Option<InputTokensDetails>,
    pub output_tokens_details: Option<OutputTokensDetails>,
}

/// Upstream `input_tokens_details`; `cache_write_tokens` is not in the public
/// OpenAI schema but upstream reads it (custom gateways report it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputTokensDetails {
    pub cached_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

/// Upstream `output_tokens_details`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputTokensDetails {
    pub reasoning_tokens: Option<u64>,
}

/// The `response` object of `response.created` / `response.completed` /
/// `response.incomplete` / `response.failed` events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResponsesResponse {
    pub id: Option<String>,
    pub status: Option<String>,
    pub error: Option<ResponseError>,
    pub incomplete_details: Option<IncompleteDetails>,
    pub usage: Option<ResponsesUsage>,
    pub service_tier: Option<String>,
    /// The terminal response's output items (raw), used for the Azure
    /// encrypted-content backfill (openai-responses-shared.ts:537-550).
    pub output: Option<Vec<Value>>,
}

impl ResponsesResponse {
    /// Defensive extraction from a raw `response` payload; every field is
    /// optional, mirroring the upstream `?.`/`|| 0` reads.
    pub fn from_raw(raw: Option<&Value>) -> Self {
        let Some(raw) = raw else {
            return Self::default();
        };
        ResponsesResponse {
            id: opt_str(raw, "id"),
            status: opt_str(raw, "status"),
            error: raw
                .get("error")
                .filter(|value| value.is_object())
                .map(|error| ResponseError {
                    code: opt_str(error, "code"),
                    message: opt_str(error, "message"),
                }),
            incomplete_details: raw
                .get("incomplete_details")
                .filter(|value| value.is_object())
                .map(|details| IncompleteDetails {
                    reason: details.get("reason").cloned(),
                }),
            usage: raw
                .get("usage")
                .filter(|value| value.is_object())
                .map(|usage| ResponsesUsage {
                    input_tokens: opt_u64(usage, "input_tokens"),
                    output_tokens: opt_u64(usage, "output_tokens"),
                    total_tokens: opt_u64(usage, "total_tokens"),
                    input_tokens_details: usage
                        .get("input_tokens_details")
                        .filter(|value| value.is_object())
                        .map(|details| InputTokensDetails {
                            cached_tokens: opt_u64(details, "cached_tokens"),
                            cache_write_tokens: opt_u64(details, "cache_write_tokens"),
                        }),
                    output_tokens_details: usage
                        .get("output_tokens_details")
                        .filter(|value| value.is_object())
                        .map(|details| OutputTokensDetails {
                            reasoning_tokens: opt_u64(details, "reasoning_tokens"),
                        }),
                }),
            service_tier: opt_str(raw, "service_tier"),
            output: raw.get("output").and_then(Value::as_array).cloned(),
        }
    }
}

/// One `response.output_item.added`/`done` item. The wire shapes the stream
/// processor consumes (`reasoning`, `message`, `function_call`,
/// `custom_tool_call`); every other item type is [`ResponsesOutputItem::Other`].
///
/// The full raw item is kept because upstream persists whole items:
/// reasoning items are stored verbatim as the thinking signature
/// (`JSON.stringify(item)`, encrypted content included).
#[derive(Debug, Clone, PartialEq)]
pub enum ResponsesOutputItem {
    Reasoning(ReasoningItem),
    Message(MessageItem),
    FunctionCall(FunctionCallItem),
    CustomToolCall(CustomToolCallItem),
    /// Any other item type (`web_search_call`, ...): no slot, no content.
    Other,
}

/// A `reasoning` output item.
#[derive(Debug, Clone, PartialEq)]
pub struct ReasoningItem {
    pub id: String,
    /// The item exactly as received (all fields preserved for replay).
    pub raw: Value,
}

impl ReasoningItem {
    /// Upstream `item.summary?.map((s) => s.text).join("\n\n") || ""`.
    fn summary_text(&self) -> String {
        join_item_texts(self.raw.get("summary"), "text")
    }

    /// Upstream `item.content?.map((c) => c.text).join("\n\n") || ""`.
    fn content_text(&self) -> String {
        join_item_texts(self.raw.get("content"), "text")
    }
}

/// Upstream array-of-`{field}` join with JS `Array.join` semantics (missing
/// fields render as empty strings).
fn join_item_texts(value: Option<&Value>, field: &str) -> String {
    let Some(Value::Array(items)) = value else {
        return String::new();
    };
    items
        .iter()
        .map(|item| item.get(field).and_then(Value::as_str).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A `message` output item.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageItem {
    pub id: String,
    /// Upstream `phase`: `"commentary"` or `"final_answer"`.
    pub phase: Option<String>,
    /// The item exactly as received.
    pub raw: Value,
}

impl MessageItem {
    /// Upstream `item.content?.map((c) => c.type === "output_text" ? c.text :
    /// c.refusal).join("")` — non-text parts contribute their `refusal`.
    fn output_text(&self) -> String {
        let Some(Value::Array(items)) = self.raw.get("content") else {
            return String::new();
        };
        items
            .iter()
            .map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("output_text") {
                    item.get("text").and_then(Value::as_str).unwrap_or("")
                } else {
                    item.get("refusal").and_then(Value::as_str).unwrap_or("")
                }
            })
            .collect::<String>()
    }
}

/// A `function_call` output item.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionCallItem {
    pub id: String,
    pub call_id: String,
    pub name: String,
    /// Upstream `item.arguments || ""` at slot creation.
    pub arguments: String,
    pub namespace: Option<String>,
}

/// A `custom_tool_call` output item.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomToolCallItem {
    pub id: String,
    pub call_id: String,
    pub name: String,
    /// Upstream reads `item.input || ""` at slot creation and
    /// `item.input ?? current` at done — `None` keeps the raw `Option`.
    pub input: Option<String>,
    pub namespace: Option<String>,
}

impl ResponsesOutputItem {
    /// Classify a raw output item; items missing required identity fields
    /// (`id`/`call_id`/`name`) are treated as unhandled.
    pub fn from_value(raw: Value) -> Self {
        let kind = raw.get("type").and_then(Value::as_str);
        match kind {
            Some("reasoning") => match raw.get("id").and_then(Value::as_str) {
                Some(id) => ResponsesOutputItem::Reasoning(ReasoningItem {
                    id: id.to_string(),
                    raw,
                }),
                None => ResponsesOutputItem::Other,
            },
            Some("message") => match raw.get("id").and_then(Value::as_str) {
                Some(id) => ResponsesOutputItem::Message(MessageItem {
                    id: id.to_string(),
                    phase: opt_str(&raw, "phase"),
                    raw,
                }),
                None => ResponsesOutputItem::Other,
            },
            Some("function_call") => {
                match (
                    raw.get("id").and_then(Value::as_str),
                    raw.get("call_id").and_then(Value::as_str),
                    raw.get("name").and_then(Value::as_str),
                ) {
                    (Some(id), Some(call_id), Some(name)) => {
                        ResponsesOutputItem::FunctionCall(FunctionCallItem {
                            id: id.to_string(),
                            call_id: call_id.to_string(),
                            name: name.to_string(),
                            arguments: opt_str(&raw, "arguments").unwrap_or_default(),
                            namespace: opt_str(&raw, "namespace"),
                        })
                    }
                    _ => ResponsesOutputItem::Other,
                }
            }
            Some("custom_tool_call") => {
                match (
                    raw.get("id").and_then(Value::as_str),
                    raw.get("call_id").and_then(Value::as_str),
                    raw.get("name").and_then(Value::as_str),
                ) {
                    (Some(id), Some(call_id), Some(name)) => {
                        ResponsesOutputItem::CustomToolCall(CustomToolCallItem {
                            id: id.to_string(),
                            call_id: call_id.to_string(),
                            name: name.to_string(),
                            input: opt_str(&raw, "input"),
                            namespace: opt_str(&raw, "namespace"),
                        })
                    }
                    _ => ResponsesOutputItem::Other,
                }
            }
            _ => ResponsesOutputItem::Other,
        }
    }
}

/// One typed Responses SSE event payload — what T8 parses each `data:` frame
/// into. Unhandled event types (everything outside the upstream switch:
/// `response.output_text.done`, `response.content_part.*`, heartbeats, ...)
/// parse as [`ResponsesStreamEvent::Unhandled`] and are ignored, mirroring
/// the upstream fall-through.
#[derive(Debug, Clone, PartialEq)]
pub enum ResponsesStreamEvent {
    ResponseCreated {
        response: ResponsesResponse,
    },
    OutputItemAdded {
        output_index: u64,
        item: ResponsesOutputItem,
    },
    OutputItemDone {
        output_index: u64,
        item: ResponsesOutputItem,
    },
    OutputTextDelta {
        output_index: u64,
        delta: String,
    },
    RefusalDelta {
        output_index: u64,
        delta: String,
    },
    ReasoningSummaryTextDelta {
        output_index: u64,
        delta: String,
    },
    ReasoningSummaryPartDone {
        output_index: u64,
    },
    ReasoningTextDelta {
        output_index: u64,
        delta: String,
    },
    FunctionCallArgumentsDelta {
        output_index: u64,
        delta: String,
    },
    FunctionCallArgumentsDone {
        output_index: u64,
        arguments: String,
    },
    CustomToolCallInputDelta {
        output_index: u64,
        delta: String,
    },
    CustomToolCallInputDone {
        output_index: u64,
        input: String,
    },
    Completed {
        response: ResponsesResponse,
    },
    Incomplete {
        response: ResponsesResponse,
    },
    Failed {
        response: ResponsesResponse,
    },
    /// The top-level `error` event (upstream throws
    /// `` `Error Code ${event.code}: ${event.message}` ``).
    Error {
        code: Option<String>,
        message: Option<String>,
    },
    /// Any other event type — ignored.
    Unhandled,
}

impl ResponsesStreamEvent {
    /// Classify a raw SSE `data:` payload (upstream switches on `event.type`).
    pub fn from_value(raw: Value) -> Self {
        let kind = raw.get("type").and_then(Value::as_str).unwrap_or("");
        let response = || ResponsesResponse::from_raw(raw.get("response"));
        let output_index = || raw.get("output_index").and_then(Value::as_u64).unwrap_or(0);
        let delta = |key: &str| opt_str(&raw, key).unwrap_or_default();
        match kind {
            "response.created" => ResponsesStreamEvent::ResponseCreated {
                response: response(),
            },
            "response.output_item.added" => ResponsesStreamEvent::OutputItemAdded {
                output_index: output_index(),
                item: ResponsesOutputItem::from_value(
                    raw.get("item").cloned().unwrap_or(Value::Null),
                ),
            },
            "response.output_item.done" => ResponsesStreamEvent::OutputItemDone {
                output_index: output_index(),
                item: ResponsesOutputItem::from_value(
                    raw.get("item").cloned().unwrap_or(Value::Null),
                ),
            },
            "response.output_text.delta" => ResponsesStreamEvent::OutputTextDelta {
                output_index: output_index(),
                delta: delta("delta"),
            },
            "response.refusal.delta" => ResponsesStreamEvent::RefusalDelta {
                output_index: output_index(),
                delta: delta("delta"),
            },
            "response.reasoning_summary_text.delta" => {
                ResponsesStreamEvent::ReasoningSummaryTextDelta {
                    output_index: output_index(),
                    delta: delta("delta"),
                }
            }
            "response.reasoning_summary_part.done" => {
                ResponsesStreamEvent::ReasoningSummaryPartDone {
                    output_index: output_index(),
                }
            }
            "response.reasoning_text.delta" => ResponsesStreamEvent::ReasoningTextDelta {
                output_index: output_index(),
                delta: delta("delta"),
            },
            "response.function_call_arguments.delta" => {
                ResponsesStreamEvent::FunctionCallArgumentsDelta {
                    output_index: output_index(),
                    delta: delta("delta"),
                }
            }
            "response.function_call_arguments.done" => {
                ResponsesStreamEvent::FunctionCallArgumentsDone {
                    output_index: output_index(),
                    arguments: delta("arguments"),
                }
            }
            "response.custom_tool_call_input.delta" => {
                ResponsesStreamEvent::CustomToolCallInputDelta {
                    output_index: output_index(),
                    delta: delta("delta"),
                }
            }
            "response.custom_tool_call_input.done" => {
                ResponsesStreamEvent::CustomToolCallInputDone {
                    output_index: output_index(),
                    input: delta("input"),
                }
            }
            "response.completed" => ResponsesStreamEvent::Completed {
                response: response(),
            },
            "response.incomplete" => ResponsesStreamEvent::Incomplete {
                response: response(),
            },
            "response.failed" => ResponsesStreamEvent::Failed {
                response: response(),
            },
            "error" => ResponsesStreamEvent::Error {
                code: opt_str(&raw, "code"),
                message: opt_str(&raw, "message"),
            },
            _ => ResponsesStreamEvent::Unhandled,
        }
    }
}

impl<'de> Deserialize<'de> for ResponsesStreamEvent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(ResponsesStreamEvent::from_value(Value::deserialize(
            deserializer,
        )?))
    }
}

// =============================================================================
// Stream processing (openai-responses-shared.ts:398-793)
// =============================================================================

/// Upstream `ResponsesOutputSlot` (lines 425-430): the open output slot for a
/// `output_index`. The content blocks themselves live in
/// `output.content[content_index]`; the tool-call slot carries the streaming
/// scratch (`partialJson` for function calls, the grammar buffer + property
/// for custom tool calls). `partial_json`/`custom_input` are mutually
/// exclusive — function-call slots always start with `partial_json: Some`,
/// custom-tool slots with `custom_input: Some`.
#[derive(Debug, Clone)]
enum OutputSlot {
    Thinking {
        content_index: usize,
    },
    Text {
        content_index: usize,
    },
    ToolCall {
        content_index: usize,
        partial_json: Option<String>,
        custom_input: Option<CustomInput>,
    },
}

/// Upstream `StreamingToolCall.customInput` (lines 402-408): the custom input
/// property and its grammar JSON buffer.
#[derive(Debug, Clone)]
struct CustomInput {
    property: String,
    buf: GrammarBuf,
}

/// Port of upstream `processResponsesStream` (lines 432-761) as a
/// push-processor: feed typed [`ResponsesStreamEvent`] payloads in stream
/// order; [`Self::output`] holds the live assistant message (the upstream
/// `output` argument) and [`AssistantMessageEvent`]s flow into the channel
/// like the other M2b API ports.
///
/// `finish` enforces the upstream post-loop guard: a stream that ends without
/// a `response.completed`/`response.incomplete`/`response.failed` event
/// fails with the upstream error message (the endpoint catch turns it into
/// the `error` event).
pub struct ResponsesStreamProcessor {
    model: Model,
    options: ResponsesStreamOptions,
    output: AssistantMessage,
    saw_terminal_response_event: bool,
    output_slots: HashMap<u64, OutputSlot>,
    /// Upstream `reasoningBlocksById`: reasoning item id → content index, for
    /// the terminal encrypted-content backfill.
    reasoning_blocks_by_id: HashMap<String, usize>,
}

impl ResponsesStreamProcessor {
    /// Seeds the upstream `output` argument: metadata with empty content,
    /// zeroed usage, `stopReason: "pending"`.
    pub fn new(model: &Model, options: ResponsesStreamOptions) -> Self {
        let output = AssistantMessage {
            content: Vec::new(),
            api: model.api.clone(),
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
        ResponsesStreamProcessor {
            model: model.clone(),
            options,
            output,
            saw_terminal_response_event: false,
            output_slots: HashMap::new(),
            reasoning_blocks_by_id: HashMap::new(),
        }
    }

    /// The live assistant message (upstream's mutated `output`).
    pub fn output(&self) -> &AssistantMessage {
        &self.output
    }

    /// Mutable access to the live assistant message: the openai-codex
    /// endpoint appends transport-failure diagnostics to the shared output
    /// mid-flight (openai-codex-responses.ts:356-365).
    pub fn output_mut(&mut self) -> &mut AssistantMessage {
        &mut self.output
    }

    /// Consumes the processor, returning the final assistant message.
    pub fn into_output(self) -> AssistantMessage {
        self.output
    }

    /// Upstream post-loop guards (lines 758-760 plus the
    /// unfinished-tool-call rejection from the Responses delta): a terminal
    /// event must have arrived, and a toolUse message must not carry a call
    /// whose output_item.done never did (its arguments may be cut off).
    pub fn finish(&self) -> Result<(), String> {
        if !self.saw_terminal_response_event {
            return Err(
                "OpenAI Responses stream ended before a terminal response event".to_string(),
            );
        }
        // The agent runs every tool call in the final message. Refuse to
        // hand over calls whose scratch buffers were never removed (their
        // arguments may be cut off or mixed up, e.g. when a non-compliant
        // server omits output_index); finished calls have their scratch
        // buffers removed.
        if self.output.stop_reason == StopReason::ToolUse {
            for slot in self.output_slots.values() {
                if let OutputSlot::ToolCall {
                    content_index,
                    partial_json,
                    custom_input,
                } = slot
                {
                    let unfinished = partial_json.as_deref().is_some_and(|json| !json.is_empty())
                        || custom_input.is_some();
                    if unfinished {
                        if let Some(AssistantBlock::ToolCall(tool_call)) =
                            self.output.content.get(*content_index)
                        {
                            return Err(format!(
                                "OpenAI Responses stream completed with an unfinished tool call: {} ({})",
                                tool_call.name, tool_call.id
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn push(&self, tx: &mpsc::Sender<AssistantMessageEvent>, event: AssistantMessageEvent) {
        let _ = tx.send(event).await;
    }

    /// Upstream `applyMessagePhaseStopReason` (lines 442-446): a message item
    /// with `phase: "final_answer"` provisionally marks the response stopped.
    fn apply_message_phase_stop_reason(&mut self, item: &ResponsesOutputItem) {
        if let ResponsesOutputItem::Message(message) = item {
            if message.phase.as_deref() == Some("final_answer") {
                self.output.stop_reason = StopReason::Stop;
            }
        }
    }

    /// Process one typed SSE event (upstream's for-await body, lines 598-756).
    /// Upstream `processResponsesStream`'s loop body: the optional
    /// `onProviderStreamEvent` observer sees each raw provider event before
    /// Pi normalization.
    pub async fn observe_raw_event(&mut self, raw: serde_json::Value) -> Result<(), String> {
        self.options
            .callbacks
            .provider_stream_event(raw, &self.model)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn process_event(
        &mut self,
        event: &ResponsesStreamEvent,
        tx: &mpsc::Sender<AssistantMessageEvent>,
    ) -> Result<(), String> {
        match event {
            ResponsesStreamEvent::ResponseCreated { response } => {
                if let Some(id) = &response.id {
                    self.output.response_id = Some(id.clone());
                }
            }
            ResponsesStreamEvent::OutputItemAdded { output_index, item } => {
                self.create_slot(*output_index, item, tx).await?;
            }
            ResponsesStreamEvent::ReasoningSummaryTextDelta {
                output_index,
                delta,
            } => {
                let Some(&OutputSlot::Thinking { content_index }) =
                    self.output_slots.get(output_index)
                else {
                    return Ok(());
                };
                if let Some(AssistantBlock::Thinking(block)) =
                    self.output.content.get_mut(content_index)
                {
                    block.thinking += delta.as_str();
                }
                self.push(
                    tx,
                    AssistantMessageEvent::ThinkingDelta {
                        content_index,
                        delta: delta.clone(),
                    },
                )
                .await;
            }
            ResponsesStreamEvent::ReasoningSummaryPartDone { output_index } => {
                let Some(&OutputSlot::Thinking { content_index }) =
                    self.output_slots.get(output_index)
                else {
                    return Ok(());
                };
                if let Some(AssistantBlock::Thinking(block)) =
                    self.output.content.get_mut(content_index)
                {
                    block.thinking += "\n\n";
                }
                self.push(
                    tx,
                    AssistantMessageEvent::ThinkingDelta {
                        content_index,
                        delta: "\n\n".to_string(),
                    },
                )
                .await;
            }
            ResponsesStreamEvent::ReasoningTextDelta {
                output_index,
                delta,
            } => {
                let Some(&OutputSlot::Thinking { content_index }) =
                    self.output_slots.get(output_index)
                else {
                    return Ok(());
                };
                if let Some(AssistantBlock::Thinking(block)) =
                    self.output.content.get_mut(content_index)
                {
                    block.thinking += delta.as_str();
                }
                self.push(
                    tx,
                    AssistantMessageEvent::ThinkingDelta {
                        content_index,
                        delta: delta.clone(),
                    },
                )
                .await;
            }
            ResponsesStreamEvent::OutputTextDelta {
                output_index,
                delta,
            }
            | ResponsesStreamEvent::RefusalDelta {
                output_index,
                delta,
            } => {
                let Some(&OutputSlot::Text { content_index }) = self.output_slots.get(output_index)
                else {
                    return Ok(());
                };
                if let Some(AssistantBlock::Text(block)) =
                    self.output.content.get_mut(content_index)
                {
                    block.text += delta.as_str();
                }
                self.push(
                    tx,
                    AssistantMessageEvent::TextDelta {
                        content_index,
                        delta: delta.clone(),
                    },
                )
                .await;
            }
            ResponsesStreamEvent::FunctionCallArgumentsDelta {
                output_index,
                delta,
            } => {
                let Some(&OutputSlot::ToolCall {
                    content_index,
                    partial_json: Some(_),
                    ..
                }) = self.output_slots.get(output_index)
                else {
                    return Ok(());
                };
                // Snapshot the extended buffer, then parse into the block.
                let partial_json = match self.output_slots.get(output_index) {
                    Some(OutputSlot::ToolCall {
                        partial_json: Some(partial_json),
                        ..
                    }) => format!("{partial_json}{delta}"),
                    _ => return Ok(()),
                };
                if let Some(OutputSlot::ToolCall {
                    partial_json: Some(stored),
                    ..
                }) = self.output_slots.get_mut(output_index)
                {
                    *stored = partial_json.clone();
                }
                let parsed = parse_streaming_json(&partial_json);
                if let Some(AssistantBlock::ToolCall(call)) =
                    self.output.content.get_mut(content_index)
                {
                    call.arguments = parsed;
                }
                self.push(
                    tx,
                    AssistantMessageEvent::ToolcallDelta {
                        content_index,
                        delta: delta.clone(),
                    },
                )
                .await;
            }
            ResponsesStreamEvent::FunctionCallArgumentsDone {
                output_index,
                arguments,
            } => {
                let Some(OutputSlot::ToolCall {
                    content_index,
                    partial_json: Some(previous_partial_json),
                    ..
                }) = self.output_slots.get(output_index)
                else {
                    return Ok(());
                };
                let content_index = *content_index;
                let previous_partial_json = previous_partial_json.clone();
                if let Some(OutputSlot::ToolCall {
                    partial_json: Some(stored),
                    ..
                }) = self.output_slots.get_mut(output_index)
                {
                    *stored = arguments.clone();
                }
                let parsed = parse_streaming_json(arguments);
                if let Some(AssistantBlock::ToolCall(call)) =
                    self.output.content.get_mut(content_index)
                {
                    call.arguments = parsed;
                }
                if arguments
                    .as_str()
                    .starts_with(previous_partial_json.as_str())
                {
                    let delta = &arguments[previous_partial_json.len()..];
                    if !delta.is_empty() {
                        self.push(
                            tx,
                            AssistantMessageEvent::ToolcallDelta {
                                content_index,
                                delta: delta.to_string(),
                            },
                        )
                        .await;
                    }
                }
            }
            ResponsesStreamEvent::CustomToolCallInputDelta {
                output_index,
                delta,
            } => {
                let Some(OutputSlot::ToolCall {
                    content_index,
                    custom_input: Some(custom),
                    ..
                }) = self.output_slots.get_mut(output_index)
                else {
                    return Ok(());
                };
                let content_index = *content_index;
                let property = custom.property.clone();
                // Current raw input mirrors in `arguments[property]`.
                let current = match self.output.content.get(content_index) {
                    Some(AssistantBlock::ToolCall(call)) => {
                        get_custom_tool_call_input(call, &property)
                    }
                    _ => String::new(),
                };
                let next_input = format!("{current}{delta}");
                let delta_out = {
                    let Some(custom) = custom_input_at(&mut self.output_slots, output_index) else {
                        return Ok(());
                    };
                    append_grammar_tool_input_json_delta(
                        &mut custom.buf,
                        &property,
                        &next_input,
                        false,
                    )?
                };
                if let Some(AssistantBlock::ToolCall(call)) =
                    self.output.content.get_mut(content_index)
                {
                    call.arguments = single_string_arguments(&property, &next_input);
                }
                if let Some(delta_out) = delta_out {
                    self.push(
                        tx,
                        AssistantMessageEvent::ToolcallDelta {
                            content_index,
                            delta: delta_out,
                        },
                    )
                    .await;
                }
            }
            ResponsesStreamEvent::CustomToolCallInputDone {
                output_index,
                input,
            } => {
                let Some(OutputSlot::ToolCall {
                    content_index,
                    custom_input: Some(custom),
                    ..
                }) = self.output_slots.get_mut(output_index)
                else {
                    return Ok(());
                };
                let content_index = *content_index;
                let property = custom.property.clone();
                let delta_out = {
                    let Some(custom) = custom_input_at(&mut self.output_slots, output_index) else {
                        return Ok(());
                    };
                    append_grammar_tool_input_json_delta(&mut custom.buf, &property, input, true)?
                };
                if let Some(AssistantBlock::ToolCall(call)) =
                    self.output.content.get_mut(content_index)
                {
                    call.arguments = single_string_arguments(&property, input);
                }
                if let Some(delta_out) = delta_out {
                    self.push(
                        tx,
                        AssistantMessageEvent::ToolcallDelta {
                            content_index,
                            delta: delta_out,
                        },
                    )
                    .await;
                }
            }
            ResponsesStreamEvent::OutputItemDone { output_index, item } => {
                self.apply_message_phase_stop_reason(item);
                // Upstream `getOrCreateSlot` (line 684): a done item without a
                // preceding added item still creates its slot (and start event).
                if !self.output_slots.contains_key(output_index) {
                    self.create_slot(*output_index, item, tx).await?;
                }
                let Some(slot) = self.output_slots.get(output_index).cloned() else {
                    return Ok(());
                };
                match (item, &slot) {
                    (
                        ResponsesOutputItem::Reasoning(reasoning),
                        OutputSlot::Thinking { content_index },
                    ) => {
                        let content_index = *content_index;
                        let summary_text = reasoning.summary_text();
                        let content_text = reasoning.content_text();
                        let Some(AssistantBlock::Thinking(block)) =
                            self.output.content.get_mut(content_index)
                        else {
                            return Ok(());
                        };
                        if !summary_text.is_empty() {
                            block.thinking = summary_text;
                        } else if !content_text.is_empty() {
                            block.thinking = content_text;
                        }
                        block.thinking_signature = Some(reasoning.raw.to_string());
                        self.reasoning_blocks_by_id
                            .insert(reasoning.id.clone(), content_index);
                        let content = block.thinking.clone();
                        self.push(
                            tx,
                            AssistantMessageEvent::ThinkingEnd {
                                content_index,
                                content,
                            },
                        )
                        .await;
                    }
                    (ResponsesOutputItem::Message(message), OutputSlot::Text { content_index }) => {
                        let content_index = *content_index;
                        let text = message.output_text();
                        let signature =
                            encode_text_signature_v1(&message.id, message.phase.as_deref());
                        let Some(AssistantBlock::Text(block)) =
                            self.output.content.get_mut(content_index)
                        else {
                            return Ok(());
                        };
                        block.text = text.clone();
                        block.text_signature = Some(signature);
                        self.push(
                            tx,
                            AssistantMessageEvent::TextEnd {
                                content_index,
                                content: text,
                            },
                        )
                        .await;
                    }
                    (
                        ResponsesOutputItem::FunctionCall(item),
                        OutputSlot::ToolCall {
                            content_index,
                            partial_json: Some(partial_json),
                            ..
                        },
                    ) => {
                        let content_index = *content_index;
                        // Upstream: `parseStreamingJson(item.arguments ||
                        // slot.block.partialJson || "{}")` — the done item's
                        // arguments win; empty strings fall through.
                        let source = if !item.arguments.is_empty() {
                            item.arguments.clone()
                        } else if !partial_json.is_empty() {
                            partial_json.clone()
                        } else {
                            "{}".to_string()
                        };
                        let parsed = parse_streaming_json(&source);
                        let Some(AssistantBlock::ToolCall(call)) =
                            self.output.content.get_mut(content_index)
                        else {
                            return Ok(());
                        };
                        // Finalize in place; the scratch buffer stays in the
                        // slot, so replay only carries parsed arguments.
                        call.arguments = parsed;
                        if let Some(namespace) = &item.namespace {
                            call.namespace = Some(namespace.clone());
                        }
                        let tool_call = call.clone();
                        self.push(
                            tx,
                            AssistantMessageEvent::ToolcallEnd {
                                content_index,
                                tool_call,
                            },
                        )
                        .await;
                    }
                    (
                        ResponsesOutputItem::CustomToolCall(item),
                        OutputSlot::ToolCall {
                            content_index,
                            custom_input: Some(custom),
                            ..
                        },
                    ) => {
                        let content_index = *content_index;
                        let property = custom.property.clone();
                        // `item.input ?? current`: an empty string input is
                        // still authoritative.
                        let next_input = match &item.input {
                            Some(input) => input.clone(),
                            None => match self.output.content.get(content_index) {
                                Some(AssistantBlock::ToolCall(call)) => {
                                    get_custom_tool_call_input(call, &property)
                                }
                                _ => String::new(),
                            },
                        };
                        let delta_out = {
                            let Some(custom) =
                                custom_input_at(&mut self.output_slots, output_index)
                            else {
                                return Ok(());
                            };
                            append_grammar_tool_input_json_delta(
                                &mut custom.buf,
                                &property,
                                &next_input,
                                true,
                            )?
                        };
                        if let Some(AssistantBlock::ToolCall(call)) =
                            self.output.content.get_mut(content_index)
                        {
                            call.arguments = single_string_arguments(&property, &next_input);
                        }
                        if let Some(delta_out) = delta_out {
                            self.push(
                                tx,
                                AssistantMessageEvent::ToolcallDelta {
                                    content_index,
                                    delta: delta_out,
                                },
                            )
                            .await;
                        }
                        if let Some(namespace) = &item.namespace {
                            if let Some(AssistantBlock::ToolCall(call)) =
                                self.output.content.get_mut(content_index)
                            {
                                call.namespace = Some(namespace.clone());
                            }
                        }
                        let tool_call = match self.output.content.get(content_index) {
                            Some(AssistantBlock::ToolCall(call)) => call.clone(),
                            _ => return Ok(()),
                        };
                        self.push(
                            tx,
                            AssistantMessageEvent::ToolcallEnd {
                                content_index,
                                tool_call,
                            },
                        )
                        .await;
                    }
                    // Item type does not match the open slot: nothing to do,
                    // and the slot stays open (upstream leaves it too).
                    _ => return Ok(()),
                }
                self.output_slots.remove(output_index);
            }
            ResponsesStreamEvent::Completed { response }
            | ResponsesStreamEvent::Incomplete { response } => {
                self.finalize_response(response)?;
            }
            ResponsesStreamEvent::Failed { response } => {
                self.saw_terminal_response_event = true;
                self.output.raw_stop_reason = response.status.clone();
                let details = response.incomplete_details.as_ref();
                let message = match &response.error {
                    Some(error) => format!(
                        "{}: {}",
                        error
                            .code
                            .as_deref()
                            .filter(|code| !code.is_empty())
                            .unwrap_or("unknown"),
                        error
                            .message
                            .as_deref()
                            .filter(|message| !message.is_empty())
                            .unwrap_or("no message")
                    ),
                    None => match details
                        .and_then(|details| details.reason.as_ref())
                        .and_then(Value::as_str)
                        .filter(|reason| !reason.is_empty())
                    {
                        Some(reason) => format!("incomplete: {reason}"),
                        None => "Unknown error (no error details in response)".to_string(),
                    },
                };
                return Err(message);
            }
            ResponsesStreamEvent::Error { code, message } => {
                // Upstream template-literal coercion: missing fields render as
                // "undefined" (the `|| "Unknown error"` fallback is dead code —
                // a template literal is never falsy).
                return Err(format!(
                    "Error Code {}: {}",
                    code.as_deref().unwrap_or("undefined"),
                    message.as_deref().unwrap_or("undefined")
                ));
            }
            // Everything else falls through upstream.
            ResponsesStreamEvent::Unhandled => {}
        }
        Ok(())
    }

    /// Upstream `createSlot` (lines 463-529): allocate the content block for
    /// a new `output_index` and push the matching `*_start` event. Unknown
    /// item types create nothing.
    async fn create_slot(
        &mut self,
        output_index: u64,
        item: &ResponsesOutputItem,
        tx: &mpsc::Sender<AssistantMessageEvent>,
    ) -> Result<(), String> {
        match item {
            ResponsesOutputItem::Reasoning(_) => {
                self.output
                    .content
                    .push(AssistantBlock::Thinking(ThinkingContent {
                        thinking: String::new(),
                        thinking_signature: None,
                        redacted: None,
                    }));
                let content_index = self.output.content.len() - 1;
                self.output_slots
                    .insert(output_index, OutputSlot::Thinking { content_index });
                self.push(tx, AssistantMessageEvent::ThinkingStart { content_index })
                    .await;
            }
            ResponsesOutputItem::Message(message) => {
                // Upstream `applyMessagePhaseStopReason` on the added item.
                if message.phase.as_deref() == Some("final_answer") {
                    self.output.stop_reason = StopReason::Stop;
                }
                self.output.content.push(AssistantBlock::Text(TextContent {
                    text: String::new(),
                    text_signature: None,
                }));
                let content_index = self.output.content.len() - 1;
                self.output_slots
                    .insert(output_index, OutputSlot::Text { content_index });
                self.push(tx, AssistantMessageEvent::TextStart { content_index })
                    .await;
            }
            ResponsesOutputItem::FunctionCall(item) => {
                // Upstream: `id: ${item.call_id}|${item.id}` with the empty
                // argument buffer seeded from the added item.
                let block = ToolCall {
                    id: format!("{}|{}", item.call_id, item.id),
                    name: item.name.clone(),
                    arguments: json!({}),
                    thought_signature: None,
                    namespace: item.namespace.clone(),
                };
                let partial_json = Some(item.arguments.clone());
                self.output.content.push(AssistantBlock::ToolCall(block));
                let content_index = self.output.content.len() - 1;
                self.output_slots.insert(
                    output_index,
                    OutputSlot::ToolCall {
                        content_index,
                        partial_json,
                        custom_input: None,
                    },
                );
                self.push(tx, AssistantMessageEvent::ToolcallStart { content_index })
                    .await;
            }
            ResponsesOutputItem::CustomToolCall(item) => {
                let input_property = self
                    .options
                    .grammar_tool_input_properties
                    .get(&item.name)
                    .cloned()
                    .unwrap_or_else(|| "input".to_string());
                let input = item.input.clone().unwrap_or_default();
                let block = ToolCall {
                    id: format!("{}|{}", item.call_id, item.id),
                    name: item.name.clone(),
                    arguments: single_string_arguments(&input_property, &input),
                    thought_signature: None,
                    namespace: item.namespace.clone(),
                };
                self.output.content.push(AssistantBlock::ToolCall(block));
                let content_index = self.output.content.len() - 1;
                self.output_slots.insert(
                    output_index,
                    OutputSlot::ToolCall {
                        content_index,
                        partial_json: None,
                        custom_input: Some(CustomInput {
                            property: input_property,
                            buf: GrammarBuf::default(),
                        }),
                    },
                );
                self.push(tx, AssistantMessageEvent::ToolcallStart { content_index })
                    .await;
            }
            ResponsesOutputItem::Other => {}
        }
        Ok(())
    }

    /// Upstream `backfillReasoningSignatures` (lines 537-550): Azure omits
    /// `reasoning.encrypted_content` from `output_item.done` and only
    /// includes it in the terminal response's output; backfill the persisted
    /// reasoning signature so store:false multi-turn replay stays stateless
    /// (pi issue #6409).
    fn backfill_reasoning_signatures(&mut self, response_output: &[Value]) {
        for item in response_output {
            if item.get("type").and_then(Value::as_str) != Some("reasoning") {
                continue;
            }
            let Some(encrypted_content) = item.get("encrypted_content").and_then(Value::as_str)
            else {
                continue;
            };
            if encrypted_content.is_empty() {
                continue;
            }
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(&content_index) = self.reasoning_blocks_by_id.get(id) else {
                continue;
            };
            let Some(AssistantBlock::Thinking(block)) = self.output.content.get_mut(content_index)
            else {
                continue;
            };
            let Some(signature) = &block.thinking_signature else {
                continue;
            };
            if signature.is_empty() {
                continue;
            }
            let Ok(mut stored_item) = serde_json::from_str::<Value>(signature) else {
                continue;
            };
            if stored_item
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_some_and(|stored| !stored.is_empty())
            {
                continue;
            }
            stored_item["encrypted_content"] = json!(encrypted_content);
            block.thinking_signature = Some(stored_item.to_string());
        }
    }

    /// Upstream `finalizeResponse` (lines 551-596).
    fn finalize_response(&mut self, response: &ResponsesResponse) -> Result<(), String> {
        self.saw_terminal_response_event = true;
        if let Some(response_output) = &response.output {
            self.backfill_reasoning_signatures(response_output);
        }
        if let Some(id) = &response.id {
            self.output.response_id = Some(id.clone());
        }
        if let Some(usage) = &response.usage {
            let cached_tokens = usage
                .input_tokens_details
                .as_ref()
                .and_then(|details| details.cached_tokens)
                .unwrap_or(0);
            let cache_write_tokens = usage
                .input_tokens_details
                .as_ref()
                .and_then(|details| details.cache_write_tokens)
                .unwrap_or(0);
            self.output.usage = Usage {
                // OpenAI includes cached and cache-write tokens in
                // input_tokens, so subtract both.
                input: usage
                    .input_tokens
                    .unwrap_or(0)
                    .saturating_sub(cached_tokens)
                    .saturating_sub(cache_write_tokens),
                output: usage.output_tokens.unwrap_or(0),
                cache_read: cached_tokens,
                cache_write: cache_write_tokens,
                cache_write_1h: None,
                reasoning: Some(
                    usage
                        .output_tokens_details
                        .as_ref()
                        .and_then(|details| details.reasoning_tokens)
                        .unwrap_or(0),
                ),
                total_tokens: usage.total_tokens.unwrap_or(0),
                cost: UsageCost::default(),
            };
        }
        calculate_cost(&self.model, &mut self.output.usage);
        if self.options.apply_service_tier_pricing.is_some() {
            let service_tier = if let Some(resolve) = self.options.resolve_service_tier.as_mut() {
                resolve(
                    response.service_tier.as_deref(),
                    self.options.service_tier.as_deref(),
                )
            } else {
                // Upstream: `response?.service_tier ?? options.serviceTier`.
                response
                    .service_tier
                    .clone()
                    .or_else(|| self.options.service_tier.clone())
            };
            if let Some(apply) = self.options.apply_service_tier_pricing.as_mut() {
                apply(&mut self.output.usage, service_tier);
            }
        }
        // Map status to stop reason. For incomplete responses, retain the
        // provider's specific reason so max-output truncation and content
        // filtering stay distinct.
        let status = response.status.as_deref();
        let incomplete_reason = response
            .incomplete_details
            .as_ref()
            .and_then(|details| details.reason.as_ref())
            .and_then(Value::as_str);
        self.output.raw_stop_reason = status.map(|status| match incomplete_reason {
            Some(reason) => format!("{status}.{reason}"),
            None => status.to_string(),
        });
        let (stop_reason, error_message) = map_stop_reason(status, incomplete_reason)?;
        self.output.stop_reason = stop_reason;
        self.output.error_message = error_message;
        if self
            .output
            .content
            .iter()
            .any(|block| matches!(block, AssistantBlock::ToolCall(_)))
            && self.output.stop_reason == StopReason::Stop
        {
            self.output.stop_reason = StopReason::ToolUse;
        }
        Ok(())
    }
}

/// Upstream `StreamingToolCall.customInput` accessor: the mutable custom
/// input state for a tool-call slot, or `None` when the slot is not a custom
/// tool call.
fn custom_input_at<'a>(
    slots: &'a mut HashMap<u64, OutputSlot>,
    output_index: &u64,
) -> Option<&'a mut CustomInput> {
    match slots.get_mut(output_index) {
        Some(OutputSlot::ToolCall {
            custom_input: Some(custom),
            ..
        }) => Some(custom),
        _ => None,
    }
}

/// Upstream `getCustomToolCallInput` (lines 410-415): the current raw custom
/// input mirrored at `arguments[property]`.
fn get_custom_tool_call_input(call: &ToolCall, property: &str) -> String {
    call.arguments
        .get(property)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// The `{ [property]: value }` argument mirror the custom-tool streaming
/// maintains (upstream `block.arguments = { [property]: nextInput }`).
fn single_string_arguments(property: &str, value: &str) -> Value {
    let mut arguments = Map::new();
    arguments.insert(property.to_string(), Value::String(value.to_string()));
    Value::Object(arguments)
}

/// Upstream `mapStopReason` (lines 763-793): provider status → stop reason.
/// `max_output_tokens` truncation maps to `length`; every other incomplete
/// reason is a non-retryable `error` with the provider reason preserved.
/// `failed`/`cancelled` are plain errors; the wonky `in_progress`/`queued`
/// statuses map to `stop`.
fn map_stop_reason(
    status: Option<&str>,
    incomplete_reason: Option<&str>,
) -> Result<(StopReason, Option<String>), String> {
    // Upstream `if (!status)` is falsy for both undefined and the empty
    // string.
    let Some(status) = status.filter(|status| !status.is_empty()) else {
        return Ok((StopReason::Stop, None));
    };
    match status {
        "completed" => Ok((StopReason::Stop, None)),
        "incomplete" => {
            if incomplete_reason == Some("max_output_tokens") {
                return Ok((StopReason::Length, None));
            }
            Ok((
                StopReason::Error,
                Some(match incomplete_reason {
                    Some(reason) => format!("Response incomplete: {reason}"),
                    None => "Response incomplete without a provider reason".to_string(),
                }),
            ))
        }
        "failed" | "cancelled" => Ok((StopReason::Error, None)),
        // These two are wonky ...
        "in_progress" | "queued" => Ok((StopReason::Stop, None)),
        // Upstream's exhaustive switch throws on unknown statuses.
        other => Err(format!("Unhandled stop reason: {other}")),
    }
}

// =============================================================================
// Session-affinity headers (upstream openai-responses.ts:50-52, 258-267)
// =============================================================================

/// Upstream `detectSessionAffinityFormat` (openai-responses.ts:50-52).
pub fn detect_session_affinity_format(model: &Model) -> SessionAffinityFormat {
    if model.provider == "openrouter" || model.base_url.contains("openrouter.ai") {
        SessionAffinityFormat::Openrouter
    } else {
        SessionAffinityFormat::Openai
    }
}

/// Upstream `createClient` session-affinity block
/// (openai-responses.ts:258-267): `openrouter` gets `x-session-id`; `openai`
/// gets both `session_id` and `x-client-request-id`; any other format
/// (`openai-nosession`) gets only `x-client-request-id`.
pub fn session_affinity_headers(
    format: SessionAffinityFormat,
    session_id: &str,
) -> Vec<(String, String)> {
    match format {
        SessionAffinityFormat::Openrouter => {
            vec![("x-session-id".to_string(), session_id.to_string())]
        }
        SessionAffinityFormat::Openai => vec![
            ("session_id".to_string(), session_id.to_string()),
            ("x-client-request-id".to_string(), session_id.to_string()),
        ],
        SessionAffinityFormat::OpenaiNosession => {
            vec![("x-client-request-id".to_string(), session_id.to_string())]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::transcript::{normalize_context, Context};
    use crate::ai::types::content::{ImageContent, TextContent};
    use crate::ai::types::message::{
        AssistantBlock, AssistantMessage, Message, StringOrBlocks, SystemMessage, TextOrImageBlock,
        ToolResultMessage, UserMessage,
    };
    use crate::ai::types::primitives::{ModelCost, StopReason, Usage, UsageCost};
    use crate::ai::types::tool::{
        ConstrainedSampling, GrammarFormat, GrammarSampling, JsonSchemaSampling, Strict, Tool,
    };
    use crate::ai::types::{Model, ModelInput};
    use serde_json::json;
    use std::collections::HashSet;

    const TS: i64 = 1758240000000;
    /// Upstream `OPENAI_TOOL_CALL_PROVIDERS` (openai-responses.ts:31).
    const OPENAI_TOOL_CALL_PROVIDERS: [&str; 3] = ["openai", "openai-codex", "opencode"];

    // ---- fixtures ----

    fn responses_model() -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: "gpt-5.4".to_string(),
            name: "GPT-5.4".to_string(),
            api: "openai-responses".to_string(),
            provider: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
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

    fn allowed() -> HashSet<String> {
        OPENAI_TOOL_CALL_PROVIDERS
            .iter()
            .map(|provider| (*provider).to_string())
            .collect()
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

    fn convert(model: &Model, ctx: &TranscriptContext) -> Vec<Value> {
        convert_responses_messages(
            model,
            ctx,
            &allowed(),
            &ConvertResponsesMessagesOptions::default(),
        )
        .unwrap()
    }

    fn convert_with(
        model: &Model,
        ctx: &TranscriptContext,
        options: &ConvertResponsesMessagesOptions,
    ) -> Vec<Value> {
        convert_responses_messages(model, ctx, &allowed(), options).unwrap()
    }

    fn user(content: &str) -> Message {
        Message::User(UserMessage {
            content: StringOrBlocks::Text(content.to_string()),
            timestamp: TS,
        })
    }

    fn system_msg(content: &str, tools_added: Option<Vec<Tool>>) -> Message {
        Message::System(SystemMessage {
            content: StringOrBlocks::Text(content.to_string()),
            sections: None,
            tools_added,
            tools_removed: None,
            timestamp: TS,
        })
    }

    fn text_block(text: &str) -> AssistantBlock {
        AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    }

    fn signed_text_block(text: &str, signature: Option<&str>) -> AssistantBlock {
        AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: signature.map(str::to_string),
        })
    }

    fn thinking_block(text: &str, signature: Option<&str>) -> AssistantBlock {
        AssistantBlock::Thinking(ThinkingContent {
            thinking: text.to_string(),
            thinking_signature: signature.map(str::to_string),
            redacted: None,
        })
    }

    fn tool_call_block(id: &str, name: &str, arguments: Value) -> AssistantBlock {
        AssistantBlock::ToolCall(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
            thought_signature: None,
            namespace: None,
        })
    }

    fn assistant_msg(
        provider: &str,
        api: &str,
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
            usage: empty_usage(),
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: TS,
        })
    }

    fn tool_result(tool_call_id: &str, content: Vec<TextOrImageBlock>) -> Message {
        tool_result_named("bash", tool_call_id, content)
    }

    fn tool_result_named(
        tool_name: &str,
        tool_call_id: &str,
        content: Vec<TextOrImageBlock>,
    ) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            content,
            details: None,
            usage: None,
            is_error: false,
            timestamp: TS,
        })
    }

    fn text_result(text: &str) -> Vec<TextOrImageBlock> {
        vec![TextOrImageBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })]
    }

    fn image_block(data: &str) -> TextOrImageBlock {
        TextOrImageBlock::Image(ImageContent {
            data: data.to_string(),
            mime_type: "image/png".to_string(),
        })
    }

    fn simple_tool(name: &str) -> Tool {
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

    // =====================================================================
    // convertResponsesMessages — upstream oracle ports
    // =====================================================================

    /// Upstream openai-responses-foreign-toolcall-id.test.ts: foreign Copilot
    /// tool item ids hash into a bounded Codex-safe `fc_<hash>` shape.
    #[test]
    fn foreign_copilot_tool_item_id_hashes_to_fc_shape() {
        const COPILOT_RAW_TOOL_CALL_ID: &str = "call_4VnzVawQXPB9MgYib7CiQFEY|I9b95oN1wD/cHXKTw3PpRkL6KkCtzTJhUxMouMWYwHeTo2j3htzfSk7YPx2vifiIM4g3A8XXyOj8q4Bt6SLUG7gqY1E3ELkrkVQNHglRfUmWj84lqxJY+Puieb3VKyX0FB+83TUzn91cDMF/4gzt990IzqVrc+nIb9RRscRD070Du16q1glydVjWR0SBJsE6TbY/esOjFpqplogQqrajm1eI++f3eLi73R6q7hVusY0QbeFySVxABCjhN0lXB04caBe1rzHjYzul6MAXj7uq+0r17VLq+yrtyYhN12wkmFqHeqTyEei6EFPbMy24Nc+IbJlkP0OCg02W+gOnyBFcbi2ctvJFSOhSjt1CqBdqCnnhwUqXjbWiT0wh3DmLScRgTHmGkaI+oAcQQjfic65nxj+TnEkReA==";
        let mut model = responses_model();
        model.provider = "openai-codex".to_string();
        model.id = "gpt-5.5".to_string();

        let context = prompt_ctx(
            "You are concise.",
            vec![
                user("Use the tool."),
                assistant_msg(
                    "github-copilot",
                    "openai-responses",
                    "gpt-5.5",
                    vec![tool_call_block(
                        COPILOT_RAW_TOOL_CALL_ID,
                        "edit",
                        json!({"path": "src/styles/app.css"}),
                    )],
                ),
                tool_result(COPILOT_RAW_TOOL_CALL_ID, text_result("ok")),
            ],
        );

        let input = convert(&model, &context);
        let function_call = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .expect("function_call item");

        // Byte-pinned against the upstream shortHash implementation.
        let expected_item_id = format!(
            "fc_{}",
            short_hash(COPILOT_RAW_TOOL_CALL_ID.split('|').nth(1).unwrap())
        );
        assert_eq!(expected_item_id, "fc_ifd2c719fz6a9");
        assert_eq!(function_call["id"], json!(expected_item_id));
        assert!(utf16_len(function_call["id"].as_str().unwrap()) <= 64);
        assert!(function_call["id"].as_str().unwrap().starts_with("fc_"));
        assert_eq!(
            function_call["call_id"],
            json!("call_4VnzVawQXPB9MgYib7CiQFEY")
        );
        assert_eq!(
            function_call["arguments"],
            json!(r#"{"path":"src/styles/app.css"}"#)
        );

        let output = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
            .expect("function_call_output item");
        assert_eq!(output["call_id"], json!("call_4VnzVawQXPB9MgYib7CiQFEY"));
        assert_eq!(output["output"], json!("ok"));
    }

    /// Upstream openai-responses-empty-tool-result.test.ts: empty tool
    /// results without images use the "(no tool output)" placeholder.
    #[test]
    fn empty_tool_result_uses_no_tool_output_placeholder() {
        let model = responses_model();
        let context = ctx_of(vec![
            user("Run the command"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![tool_call_block(
                    "tool-1",
                    "bash",
                    json!({"command": "true"}),
                )],
            ),
            tool_result(
                "tool-1",
                vec![TextOrImageBlock::Text(TextContent {
                    text: String::new(),
                    text_signature: None,
                })],
            ),
        ]);

        let input = convert(&model, &context);
        let function_call_output = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
            .expect("function_call_output item");
        assert_eq!(function_call_output["output"], json!("(no tool output)"));
        assert!(!function_call_output["output"]
            .as_str()
            .unwrap()
            .contains("see attached image"));
    }

    /// Upstream openai-responses-tool-result-images.test.ts: tool result
    /// images stay inside the function_call_output as input_image items.
    #[test]
    fn tool_result_images_become_input_image_items() {
        let mut model = responses_model();
        model.input = vec![ModelInput::Text, ModelInput::Image];
        let context = ctx_of(vec![
            user("Call the tool"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![tool_call_block("call_img|fc_img", "get_circle", json!({}))],
            ),
            tool_result(
                "call_img|fc_img",
                vec![
                    TextOrImageBlock::Text(TextContent {
                        text: "A red circle with a diameter of 100 pixels.".to_string(),
                        text_signature: None,
                    }),
                    image_block("aGVsbG8="),
                ],
            ),
        ]);

        let input = convert(&model, &context);
        let function_call_output = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
            .expect("function_call_output item");
        let output = function_call_output["output"]
            .as_array()
            .expect("array output");
        assert_eq!(function_call_output["call_id"], json!("call_img"));
        let text_item = output
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("input_text"))
            .expect("input_text item");
        assert_eq!(
            text_item["text"],
            json!("A red circle with a diameter of 100 pixels.")
        );
        let image_item = output
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("input_image"))
            .expect("input_image item");
        assert_eq!(
            image_item["image_url"],
            json!("data:image/png;base64,aGVsbG8=")
        );
        assert_eq!(image_item["detail"], json!("auto"));
    }

    /// Upstream convertToolResultOutput (lines 80-107): the string
    /// placeholder path only applies when the model cannot take images;
    /// image-only results on image-capable models become input_image arrays.
    #[test]
    fn tool_result_image_placeholders() {
        let model = responses_model();
        let mut image_model = responses_model();
        image_model.input = vec![ModelInput::Text, ModelInput::Image];

        // Image-only on an image model: an input_image array, no text part.
        assert_eq!(
            convert_tool_result_output(&image_model, &[image_block("aGVsbG8=")]),
            json!([{"type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,aGVsbG8="}])
        );
        // Images on a non-image model: "(see attached image)".
        assert_eq!(
            convert_tool_result_output(&model, &[image_block("aGVsbG8=")]),
            json!("(see attached image)")
        );
        // Multi-line text results join with "\n".
        let two_lines = vec![
            TextOrImageBlock::Text(TextContent {
                text: "one".to_string(),
                text_signature: None,
            }),
            TextOrImageBlock::Text(TextContent {
                text: "two".to_string(),
                text_signature: None,
            }),
        ];
        assert_eq!(
            convert_tool_result_output(&model, &two_lines),
            json!("one\ntwo")
        );
    }

    /// Upstream openai-responses-message-id.test.ts: multiple text blocks in
    /// one assistant turn get unique fallback message ids.
    #[test]
    fn fallback_message_ids_are_unique_per_text_block() {
        let mut model = responses_model();
        model.provider = "openai-codex".to_string();
        model.id = "gpt-5.5".to_string();
        let context = prompt_ctx(
            "You are concise.",
            vec![
                user("hello"),
                assistant_msg(
                    "anthropic",
                    "anthropic-messages",
                    "claude-opus-4-8",
                    vec![
                        thinking_block("private reasoning", None),
                        text_block("visible answer"),
                    ],
                ),
            ],
        );

        let input = convert(&model, &context);
        let message_ids: Vec<&str> = input
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
            .filter_map(|item| item.get("id").and_then(Value::as_str))
            .collect();
        assert_eq!(message_ids, ["msg_pi_1", "msg_pi_1_1"]);
        assert_eq!(
            std::collections::HashSet::<&str>::from_iter(message_ids.iter().copied()).len(),
            message_ids.len()
        );
    }

    /// Upstream lines 267-287: text signatures preserve ids and phases; the
    /// legacy plain-string form still yields the id.
    #[test]
    fn text_signature_preserves_id_and_phase() {
        let model = responses_model();
        let context = ctx_of(vec![
            user("hello"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![
                    signed_text_block(
                        "commentary",
                        Some(r#"{"v":1,"id":"msg_abc","phase":"commentary"}"#),
                    ),
                    signed_text_block("legacy", Some("msg_legacy_id")),
                ],
            ),
        ]);

        let input = convert(&model, &context);
        let messages: Vec<&Value> = input
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["id"], json!("msg_abc"));
        assert_eq!(messages[0]["phase"], json!("commentary"));
        assert_eq!(messages[0]["role"], json!("assistant"));
        assert_eq!(messages[0]["status"], json!("completed"));
        assert_eq!(
            messages[0]["content"],
            json!([{"type": "output_text", "text": "commentary", "annotations": []}])
        );
        assert_eq!(messages[1]["id"], json!("msg_legacy_id"));
        // Legacy signatures carry no phase.
        assert!(messages[1].get("phase").is_none());
    }

    /// Upstream line 278: ids over the 64-char limit are re-hashed.
    #[test]
    fn overlong_message_id_is_hashed() {
        let model = responses_model();
        let long_id = "msg_".to_string() + &"a".repeat(80);
        let context = ctx_of(vec![
            user("hello"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![signed_text_block("text", Some(&long_id))],
            ),
        ]);

        let input = convert(&model, &context);
        let message = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("message"))
            .unwrap();
        assert_eq!(
            message["id"],
            json!(format!("msg_{}", short_hash(&long_id)))
        );
    }

    /// Upstream lines 288-326: same-model function_call replay keeps the
    /// `fc_*` item id and the namespace; different-model same-api replay
    /// drops the fc_* id to avoid the reasoning pairing validation.
    #[test]
    fn function_call_replay_id_rules() {
        let model = responses_model();
        let namespace_call = ToolCall {
            id: "call_x|fc_y".to_string(),
            name: "lookup".to_string(),
            arguments: json!({"value": "hello"}),
            thought_signature: None,
            namespace: Some("dynamic_tools".to_string()),
        };

        // Same model: id and namespace preserved.
        let context = ctx_of(vec![
            user("hi"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![AssistantBlock::ToolCall(namespace_call.clone())],
            ),
        ]);
        let input = convert(&model, &context);
        let function_call = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .unwrap();
        assert_eq!(function_call["id"], json!("fc_y"));
        assert_eq!(function_call["call_id"], json!("call_x"));
        assert_eq!(function_call["arguments"], json!(r#"{"value":"hello"}"#));
        assert_eq!(function_call["namespace"], json!("dynamic_tools"));

        // Same provider/api, different model: id and namespace dropped.
        let mut other_model = responses_model();
        other_model.id = "gpt-5.2".to_string();
        let context = ctx_of(vec![
            user("hi"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![AssistantBlock::ToolCall(namespace_call.clone())],
            ),
        ]);
        let input = convert(&other_model, &context);
        let function_call = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .unwrap();
        assert!(function_call.get("id").is_none());
        assert!(function_call.get("namespace").is_none());
        assert_eq!(function_call["call_id"], json!("call_x"));
    }

    /// Upstream lines 262-266 and transform-messages: signed same-model
    /// thinking replays as the raw reasoning item (encrypted content
    /// included); unsigned thinking is not replayed at all.
    #[test]
    fn reasoning_replay_rules() {
        let model = responses_model();
        let signature = json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"type": "summary_text", "text": "S"}],
            "encrypted_content": "ENC"
        })
        .to_string();

        // Same model, signed: replayed verbatim.
        let context = ctx_of(vec![
            user("hi"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![thinking_block("thinking text", Some(&signature))],
            ),
        ]);
        let input = convert(&model, &context);
        let reasoning = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
            .expect("reasoning item");
        assert_eq!(
            reasoning,
            &json!({
                "type": "reasoning",
                "id": "rs_1",
                "summary": [{"type": "summary_text", "text": "S"}],
                "encrypted_content": "ENC"
            })
        );

        // Same model, unsigned: nothing replayed for the thinking block.
        let context = ctx_of(vec![
            user("hi"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![thinking_block("thinking text", None)],
            ),
        ]);
        let input = convert(&model, &context);
        assert!(input
            .iter()
            .all(|item| item.get("type").and_then(Value::as_str) != Some("reasoning")));
        // No message item either: the assistant produced no convertible blocks.
        assert!(input
            .iter()
            .all(|item| item.get("type").and_then(Value::as_str) != Some("message")));
    }

    /// Upstream openai-responses-namespace.test.ts: a function namespace
    /// received only on output_item.done round-trips through the stream
    /// processor and the replay conversion.
    #[tokio::test]
    async fn function_namespace_round_trips() {
        let model = responses_model();
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "function_call",
                    "id": "fc_test",
                    "call_id": "call_test",
                    "name": "lookup",
                    "arguments": ""
                })),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "function_call",
                    "id": "fc_test",
                    "call_id": "call_test",
                    "name": "lookup",
                    "arguments": "{\"value\":\"hello\"}",
                    "namespace": "dynamic_tools"
                })),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test",
                    "status": "completed"
                }))),
            },
        ];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;

        let tool_call = match output.content.first() {
            Some(AssistantBlock::ToolCall(call)) => call.clone(),
            other => panic!("expected toolCall block, got {other:?}"),
        };
        assert_eq!(tool_call.id, "call_test|fc_test");
        assert_eq!(tool_call.name, "lookup");
        assert_eq!(tool_call.arguments, json!({"value": "hello"}));
        assert_eq!(tool_call.namespace.as_deref(), Some("dynamic_tools"));

        let context = ctx_of(vec![Message::Assistant(output.clone())]);
        let input = convert(&model, &context);
        let replayed = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .unwrap();
        assert_eq!(replayed["id"], json!("fc_test"));
        assert_eq!(replayed["call_id"], json!("call_test"));
        assert_eq!(replayed["name"], json!("lookup"));
        assert_eq!(replayed["arguments"], json!(r#"{"value":"hello"}"#));
        assert_eq!(replayed["namespace"], json!("dynamic_tools"));
    }

    /// Upstream openai-responses-namespace.test.ts: a custom-tool namespace
    /// received only on output_item.done round-trips the same way.
    #[tokio::test]
    async fn custom_tool_namespace_round_trips() {
        let model = responses_model();
        let mut options = ResponsesStreamOptions::default();
        options
            .grammar_tool_input_properties
            .insert("query".to_string(), "input".to_string());
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "custom_tool_call",
                    "id": "ctc_test",
                    "call_id": "call_test",
                    "name": "query",
                    "input": ""
                })),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "custom_tool_call",
                    "id": "ctc_test",
                    "call_id": "call_test",
                    "name": "query",
                    "input": "hello",
                    "namespace": "dynamic_tools"
                })),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test",
                    "status": "completed"
                }))),
            },
        ];
        let (output, _) = run_events(&model, options, events).await;

        let tool_call = match output.content.first() {
            Some(AssistantBlock::ToolCall(call)) => call.clone(),
            other => panic!("expected toolCall block, got {other:?}"),
        };
        assert_eq!(tool_call.id, "call_test|ctc_test");
        assert_eq!(tool_call.name, "query");
        assert_eq!(tool_call.arguments, json!({"input": "hello"}));
        assert_eq!(tool_call.namespace.as_deref(), Some("dynamic_tools"));

        let mut convert_options = ConvertResponsesMessagesOptions::default();
        convert_options
            .grammar_tool_input_properties
            .insert("query".to_string(), "input".to_string());
        let context = ctx_of(vec![Message::Assistant(output.clone())]);
        let input = convert_with(&model, &context, &convert_options);
        let replayed = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("custom_tool_call"))
            .unwrap();
        assert_eq!(replayed["id"], json!("ctc_test"));
        assert_eq!(replayed["call_id"], json!("call_test"));
        assert_eq!(replayed["name"], json!("query"));
        assert_eq!(replayed["input"], json!("hello"));
        assert_eq!(replayed["namespace"], json!("dynamic_tools"));
    }

    /// Upstream openai-responses-namespace.test.ts: namespaces are dropped
    /// when the target model cannot replay the load items (different model,
    /// provider, or api).
    #[test]
    fn namespaces_dropped_for_foreign_targets() {
        let output = assistant_msg(
            "openai",
            "openai-responses",
            "gpt-5.4",
            vec![
                AssistantBlock::ToolCall(ToolCall {
                    id: "call_function|fc_test".to_string(),
                    name: "lookup".to_string(),
                    arguments: json!({"value": "hello"}),
                    thought_signature: None,
                    namespace: Some("dynamic_tools".to_string()),
                }),
                AssistantBlock::ToolCall(ToolCall {
                    id: "call_custom|ctc_test".to_string(),
                    name: "query".to_string(),
                    arguments: json!({"input": "hello"}),
                    thought_signature: None,
                    namespace: Some("dynamic_tools".to_string()),
                }),
            ],
        );
        let mut azure = responses_model();
        azure.provider = "azure-openai-responses".to_string();
        let mut codex = responses_model();
        codex.api = "openai-codex-responses".to_string();
        codex.provider = "openai-codex".to_string();
        codex.id = "gpt-5.3-codex-spark".to_string();
        let mut older = responses_model();
        older.id = "gpt-5.2".to_string();
        older.name = "GPT-5.2".to_string();

        let mut convert_options = ConvertResponsesMessagesOptions::default();
        convert_options
            .grammar_tool_input_properties
            .insert("query".to_string(), "input".to_string());

        for target in [&azure, &codex, &older] {
            let context = ctx_of(vec![output.clone()]);
            let input =
                convert_responses_messages(target, &context, &allowed(), &convert_options).unwrap();
            for kind in ["function_call", "custom_tool_call"] {
                let item = input
                    .iter()
                    .find(|item| item.get("type").and_then(Value::as_str) == Some(kind))
                    .unwrap_or_else(|| panic!("{kind} item"));
                assert!(item.get("namespace").is_none(), "{kind}: {item}");
            }
        }
    }

    /// Upstream openai-responses-namespace.test.ts: ordinary function calls
    /// never grow a namespace.
    #[test]
    fn ordinary_function_calls_have_no_namespace() {
        let model = responses_model();
        let context = ctx_of(vec![
            user("hi"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![tool_call_block(
                    "call_test|fc_test",
                    "lookup",
                    json!({"value": "hello"}),
                )],
            ),
        ]);
        let input = convert(&model, &context);
        let replayed = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .unwrap();
        assert!(replayed.get("namespace").is_none());
    }

    /// Upstream lines 211-226: the leading system message becomes a
    /// `developer` instruction for reasoning models (unless
    /// `supportsDeveloperRole` is false) and a `system` instruction otherwise;
    /// `includeSystemPrompt: false` omits it.
    #[test]
    fn system_prompt_instruction_roles() {
        let mut model = responses_model();
        let context = prompt_ctx("You are concise.", vec![user("hello")]);

        // Reasoning model without compat: developer role.
        let input = convert(&model, &context);
        assert_eq!(
            input[0],
            json!({"role": "developer", "content": "You are concise."})
        );

        // supportsDeveloperRole false: system role.
        model.compat = Some(json!({"supportsDeveloperRole": false}));
        let input = convert(&model, &context);
        assert_eq!(
            input[0],
            json!({"role": "system", "content": "You are concise."})
        );

        // Non-reasoning model: system role.
        model.compat = None;
        model.reasoning = false;
        let input = convert(&model, &context);
        assert_eq!(
            input[0],
            json!({"role": "system", "content": "You are concise."})
        );

        // includeSystemPrompt false: the leading instruction is omitted (the
        // user item still emits).
        model.reasoning = true;
        let options = ConvertResponsesMessagesOptions {
            include_system_prompt: Some(false),
            ..Default::default()
        };
        let input = convert_with(&model, &context, &options);
        assert!(input
            .iter()
            .all(|item| item.get("role").is_none() || item["role"] == json!("user")));
    }

    /// Upstream lines 217-226: mid-conversation system messages render their
    /// update text in place (requires `supportsMidConvoSystemMessages`).
    #[test]
    fn mid_conversation_system_renders_update() {
        let model = responses_model();
        let context = ctx_of(vec![
            user("hello"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![text_block("hi")],
            ),
            system_msg("New instructions", None),
            user("bye"),
        ]);
        let options = ConvertResponsesMessagesOptions {
            supports_mid_convo_system_messages: true,
            ..Default::default()
        };
        let input = convert_with(&model, &context, &options);
        let update = input
            .iter()
            .find(|item| item.get("content") == Some(&json!("New instructions")))
            .expect("system update item");
        assert_eq!(update["role"], json!("developer"));
    }

    /// Upstream appendSystemToolAdditions (lines 182-210): mid-conversation
    /// tool additions become tool_search_call/output items under
    /// `supportsToolSearch`, and `additional_tools` items under
    /// `supportsAdditionalTools`.
    #[test]
    fn mid_conversation_tool_additions() {
        let model = responses_model();
        let tools = vec![simple_tool("read"), simple_tool("write")];
        let context = ctx_of(vec![
            user("hello"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![text_block("hi")],
            ),
            system_msg("Also use these.", Some(tools)),
            user("bye"),
        ]);

        // Tool search path: call_id is `pi_tool_load_<hash(seed:names)>`.
        let options = ConvertResponsesMessagesOptions {
            supports_mid_convo_system_messages: true,
            supports_tool_search: true,
            ..Default::default()
        };
        let input = convert_with(&model, &context, &options);
        let search_call = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("tool_search_call"))
            .expect("tool_search_call item");
        let expected_call_id = format!("pi_tool_load_{}", short_hash("system:2:read,write"));
        assert_eq!(search_call["call_id"], json!(expected_call_id));
        assert_eq!(search_call["execution"], json!("client"));
        assert_eq!(search_call["status"], json!("completed"));
        assert_eq!(
            search_call["arguments"],
            json!({"query": "read write", "limit": 2})
        );
        let search_output = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("tool_search_output"))
            .expect("tool_search_output item");
        assert_eq!(search_output["call_id"], search_call["call_id"]);
        let tools_out = search_output["tools"].as_array().expect("tools array");
        assert_eq!(tools_out.len(), 2);
        // Tool-search results are deferred.
        assert_eq!(tools_out[0]["defer_loading"], json!(true));
        assert_eq!(tools_out[0]["type"], json!("function"));

        // Additional tools path.
        let options = ConvertResponsesMessagesOptions {
            supports_mid_convo_system_messages: true,
            supports_additional_tools: true,
            ..Default::default()
        };
        let input = convert_with(&model, &context, &options);
        let additional = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("additional_tools"))
            .expect("additional_tools item");
        assert_eq!(additional["role"], json!("developer"));
        assert_eq!(additional["tools"].as_array().unwrap().len(), 2);
        assert!(input
            .iter()
            .all(|item| item.get("type").and_then(Value::as_str) != Some("tool_search_call")));

        // Neither support: no tool items at all.
        let input = convert(&model, &context);
        assert!(input.iter().all(|item| {
            !matches!(
                item.get("type").and_then(Value::as_str),
                Some("tool_search_call") | Some("tool_search_output") | Some("additional_tools")
            )
        }));
    }

    /// Upstream lines 227-252: user messages convert to input_text /
    /// input_image items; empty block arrays drop the message without
    /// consuming a message index.
    #[test]
    fn user_message_conversion() {
        let mut model = responses_model();
        model.input = vec![ModelInput::Text, ModelInput::Image];
        let context = normalize_context(&Context {
            system_prompt: None,
            messages: vec![
                Message::User(UserMessage {
                    content: StringOrBlocks::Blocks(vec![
                        TextOrImageBlock::Text(TextContent {
                            text: "look".to_string(),
                            text_signature: None,
                        }),
                        image_block("aGVsbG8="),
                    ]),
                    timestamp: TS,
                }),
                user("next"),
            ],
            tools: None,
        });
        let input = convert(&model, &context);
        assert_eq!(
            input[0],
            json!({
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "look"},
                    {"type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,aGVsbG8="}
                ]
            })
        );
        assert_eq!(
            input[1],
            json!({"role": "user", "content": [{"type": "input_text", "text": "next"}]})
        );

        // An empty user block array produces no item and does not consume a
        // message index: the next user consumes index 0, so the assistant
        // text block gets `msg_pi_1` (not `msg_pi_2`).
        let context = ctx_of(vec![
            Message::User(UserMessage {
                content: StringOrBlocks::Blocks(vec![]),
                timestamp: TS,
            }),
            Message::User(UserMessage {
                content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                    text: "real".to_string(),
                    text_signature: None,
                })]),
                timestamp: TS,
            }),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![text_block("ok")],
            ),
        ]);
        let input = convert(&model, &context);
        let message = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("message"))
            .unwrap();
        assert_eq!(message["id"], json!("msg_pi_1"));
    }

    /// Upstream lines 331-347: tool results split the pipe pair and choose
    /// the output item type by the grammar properties map.
    #[test]
    fn tool_result_output_items() {
        let model = responses_model();
        let context = ctx_of(vec![
            user("hi"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![
                    tool_call_block("call_a|fc_b", "bash", json!({})),
                    tool_call_block("call_c|ctc_d", "query", json!({"input": "x"})),
                ],
            ),
            tool_result("call_a|fc_b", text_result("ok")),
            tool_result_named("query", "call_c|ctc_d", text_result("ran")),
        ]);
        let options = ConvertResponsesMessagesOptions {
            grammar_tool_input_properties: HashMap::from([(
                "query".to_string(),
                "input".to_string(),
            )]),
            ..Default::default()
        };
        let input = convert_with(&model, &context, &options);

        let function_output = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
            .unwrap();
        assert_eq!(function_output["call_id"], json!("call_a"));
        assert_eq!(function_output["output"], json!("ok"));
        let custom_output = input
            .iter()
            .find(|item| {
                item.get("type").and_then(Value::as_str) == Some("custom_tool_call_output")
            })
            .unwrap();
        assert_eq!(custom_output["call_id"], json!("call_c"));
        assert_eq!(custom_output["output"], json!("ran"));
    }

    /// Upstream lines 306-316: replayed custom tool calls read the raw input
    /// from the grammar property and keep the ctc_* item id.
    #[test]
    fn custom_tool_call_replay() {
        let model = responses_model();
        let context = ctx_of(vec![
            user("hi"),
            assistant_msg(
                "openai",
                "openai-responses",
                "gpt-5.4",
                vec![AssistantBlock::ToolCall(ToolCall {
                    id: "call_q|ctc_1".to_string(),
                    name: "query".to_string(),
                    arguments: json!({"input": "SELECT 1"}),
                    thought_signature: None,
                    namespace: None,
                })],
            ),
        ]);
        let mut options = ConvertResponsesMessagesOptions::default();
        options
            .grammar_tool_input_properties
            .insert("query".to_string(), "input".to_string());
        let input = convert_with(&model, &context, &options);
        let custom = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("custom_tool_call"))
            .unwrap();
        assert_eq!(custom["id"], json!("ctc_1"));
        assert_eq!(custom["call_id"], json!("call_q"));
        assert_eq!(custom["name"], json!("query"));
        assert_eq!(custom["input"], json!("SELECT 1"));
    }

    // =====================================================================
    // convertResponsesTools — upstream oracle ports
    // =====================================================================

    /// Upstream convertResponsesTools (lines 359-396) + openai-responses.ts
    /// callers: default strict false, explicit null, strict mode support, and
    /// schema transformation for constrained strict tools.
    #[test]
    fn function_tool_strict_variants() {
        let tool = simple_tool("read");

        // Default (no options): strict false.
        let converted = convert_responses_tools(
            std::slice::from_ref(&tool),
            &ConvertResponsesToolsOptions::default(),
        )
        .unwrap();
        assert_eq!(
            converted[0],
            json!({
                "type": "function",
                "name": "read",
                "description": "Tool read",
                "parameters": tool.parameters,
                "strict": false
            })
        );

        // supportsStrictMode false: the strict key is omitted entirely.
        let options = ConvertResponsesToolsOptions {
            supports_strict_mode: false,
            ..ConvertResponsesToolsOptions::default()
        };
        let converted = convert_responses_tools(std::slice::from_ref(&tool), &options).unwrap();
        assert!(converted[0].get("strict").is_none());

        // strict: null (codex endpoint) serializes JSON null.
        let options = ConvertResponsesToolsOptions {
            strict: Some(None),
            ..ConvertResponsesToolsOptions::default()
        };
        let converted = convert_responses_tools(std::slice::from_ref(&tool), &options).unwrap();
        assert_eq!(converted[0]["strict"], Value::Null);

        // Constrained strict tool: strict true + transformed schema. Required
        // properties stay as-is; optional non-nullable properties are
        // nullable-ized and every property becomes required.
        let mut strict_tool = simple_tool("read");
        strict_tool.parameters = json!({
            "type": "object",
            "properties": {
                "value": {"type": "string"},
                "note": {"type": "string"}
            },
            "required": ["value"]
        });
        strict_tool.constrained_sampling =
            Some(ConstrainedSampling::JsonSchema(JsonSchemaSampling {
                strict: Strict::Prefer,
            }));
        let converted =
            convert_responses_tools(&[strict_tool], &ConvertResponsesToolsOptions::default())
                .unwrap();
        assert_eq!(converted[0]["strict"], json!(true));
        let parameters = &converted[0]["parameters"];
        assert_eq!(parameters["additionalProperties"], json!(false));
        assert_eq!(
            parameters["required"],
            json!(["value", "note"]),
            "strict schemas require every property in upstream insertion order"
        );
        assert_eq!(
            parameters["properties"]["value"],
            json!({"type": "string"}),
            "already-required properties stay untouched"
        );
        assert_eq!(
            parameters["properties"]["note"],
            json!({"anyOf": [{"type": "string"}, {"type": "null"}]})
        );
    }

    /// Upstream lines 364-377: grammar tools become `custom` tools with the
    /// lark/regex grammar format and defer_loading on tool-search results.
    /// Grammar support is an option (upstream default false).
    #[test]
    fn grammar_tools_become_custom_tools() {
        let mut tool = simple_tool("write");
        tool.parameters = json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"]
        });
        tool.constrained_sampling = Some(ConstrainedSampling::Grammar(GrammarSampling {
            variants: [(GrammarFormat::Lark, "grammar".to_string())]
                .into_iter()
                .collect(),
        }));

        // Without grammar support the tool stays a plain function tool.
        let converted = convert_responses_tools(
            std::slice::from_ref(&tool),
            &ConvertResponsesToolsOptions::default(),
        )
        .unwrap();
        assert_eq!(converted[0]["type"], json!("function"));

        let grammar_options = ConvertResponsesToolsOptions {
            supports_openai_grammar_tools: true,
            ..ConvertResponsesToolsOptions::default()
        };
        let converted =
            convert_responses_tools(std::slice::from_ref(&tool), &grammar_options).unwrap();
        assert_eq!(
            converted[0],
            json!({
                "type": "custom",
                "name": "write",
                "description": "Tool write",
                "format": {"type": "grammar", "syntax": "lark", "definition": "grammar"}
            })
        );

        let options = ConvertResponsesToolsOptions {
            supports_openai_grammar_tools: true,
            tool_search_result: true,
            ..ConvertResponsesToolsOptions::default()
        };
        let converted = convert_responses_tools(&[tool], &options).unwrap();
        assert_eq!(converted[0]["defer_loading"], json!(true));
    }

    // =====================================================================
    // processResponsesStream — upstream oracle ports
    // =====================================================================

    async fn run_events(
        model: &Model,
        options: ResponsesStreamOptions,
        events: Vec<ResponsesStreamEvent>,
    ) -> (AssistantMessage, Vec<AssistantMessageEvent>) {
        let mut processor = ResponsesStreamProcessor::new(model, options);
        let (tx, mut rx) = mpsc::channel(64);
        for event in &events {
            processor.process_event(event, &tx).await.unwrap();
        }
        processor.finish().unwrap();
        let mut emitted = Vec::new();
        while let Ok(event) = rx.try_recv() {
            emitted.push(event);
        }
        (processor.into_output(), emitted)
    }

    /// Collects the live stop reason after each event (the port's observable
    /// replacement for upstream's `event.partial.stopReason` assertions).
    async fn run_events_tracked(
        model: &Model,
        options: ResponsesStreamOptions,
        events: Vec<ResponsesStreamEvent>,
    ) -> (AssistantMessage, Vec<StopReason>) {
        let mut processor = ResponsesStreamProcessor::new(model, options);
        let (tx, mut rx) = mpsc::channel(64);
        let mut stop_reasons = Vec::new();
        for event in &events {
            processor.process_event(event, &tx).await.unwrap();
            stop_reasons.push(processor.output().stop_reason);
        }
        processor.finish().unwrap();
        while let Ok(_event) = rx.try_recv() {}
        (processor.into_output(), stop_reasons)
    }

    /// Upstream openai-responses-terminal-event.test.ts "finalizes completed
    /// terminal events as stop": usage math (cached + cache-write subtracted
    /// from input), rawStopReason, responseId.
    #[tokio::test]
    async fn completed_terminal_finalizes_usage_and_stop() {
        let model = responses_model();
        let events = vec![ResponsesStreamEvent::Completed {
            response: ResponsesResponse::from_raw(Some(&json!({
                "id": "resp_completed",
                "status": "completed",
                "usage": {
                    "input_tokens": 20,
                    "output_tokens": 7,
                    "total_tokens": 27,
                    "input_tokens_details": {"cached_tokens": 2, "cache_write_tokens": 3}
                }
            }))),
        }];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;

        assert_eq!(output.response_id.as_deref(), Some("resp_completed"));
        assert_eq!(output.stop_reason, StopReason::Stop);
        assert_eq!(output.raw_stop_reason.as_deref(), Some("completed"));
        assert!(output.error_message.is_none());
        assert_eq!(output.usage.input, 15);
        assert_eq!(output.usage.output, 7);
        assert_eq!(output.usage.cache_read, 2);
        assert_eq!(output.usage.cache_write, 3);
        assert_eq!(output.usage.reasoning, Some(0));
        assert_eq!(output.usage.total_tokens, 27);
    }

    /// Upstream "finalizes incomplete terminal events as length stops".
    #[tokio::test]
    async fn incomplete_max_output_tokens_maps_to_length() {
        let model = responses_model();
        let events = vec![ResponsesStreamEvent::Incomplete {
            response: ResponsesResponse::from_raw(Some(&json!({
                "id": "resp_incomplete",
                "status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"},
                "usage": {
                    "input_tokens": 30,
                    "output_tokens": 12,
                    "total_tokens": 42,
                    "input_tokens_details": {"cached_tokens": 5}
                }
            }))),
        }];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;

        assert_eq!(output.response_id.as_deref(), Some("resp_incomplete"));
        assert_eq!(output.stop_reason, StopReason::Length);
        assert_eq!(
            output.raw_stop_reason.as_deref(),
            Some("incomplete.max_output_tokens")
        );
        assert_eq!(output.usage.input, 25);
        assert_eq!(output.usage.output, 12);
        assert_eq!(output.usage.cache_read, 5);
        assert_eq!(output.usage.cache_write, 0);
        assert_eq!(output.usage.total_tokens, 42);
    }

    /// Upstream "finalizes content-filtered incomplete responses as
    /// non-retryable errors" + "preserves unknown provider incomplete
    /// reasons".
    #[tokio::test]
    async fn incomplete_other_reasons_map_to_error() {
        let model = responses_model();
        for reason in ["content_filter", "max_time_limit"] {
            let events = vec![ResponsesStreamEvent::Incomplete {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_incomplete",
                    "status": "incomplete",
                    "incomplete_details": {"reason": reason}
                }))),
            }];
            let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;
            assert_eq!(output.stop_reason, StopReason::Error, "reason: {reason}");
            assert_eq!(
                output.raw_stop_reason.as_deref(),
                Some(format!("incomplete.{reason}").as_str())
            );
            assert_eq!(
                output.error_message.as_deref(),
                Some(format!("Response incomplete: {reason}").as_str())
            );
        }
    }

    /// Upstream "rejects failed terminal events with the provider error":
    /// `response.failed` throws with `code: message` after stamping
    /// rawStopReason, and counts as a terminal event.
    #[tokio::test]
    async fn failed_terminal_rejects_with_provider_error() {
        let model = responses_model();
        let mut processor =
            ResponsesStreamProcessor::new(&model, ResponsesStreamOptions::default());
        let (tx, _rx) = mpsc::channel(64);
        let error = processor
            .process_event(
                &ResponsesStreamEvent::Failed {
                    response: ResponsesResponse::from_raw(Some(&json!({
                        "id": "resp_failed",
                        "status": "failed",
                        "error": {"code": "server_error", "message": "boom"}
                    }))),
                },
                &tx,
            )
            .await
            .unwrap_err();
        assert_eq!(error, "server_error: boom");
        assert_eq!(
            processor.output().raw_stop_reason.as_deref(),
            Some("failed")
        );
        // A failed event is terminal: finish must not add the EOF error.
        processor.finish().unwrap();
    }

    /// Upstream "rejects streams that end before a terminal response event".
    #[tokio::test]
    async fn stream_without_terminal_event_is_rejected() {
        let model = responses_model();
        let events = vec![
            ResponsesStreamEvent::ResponseCreated {
                response: ResponsesResponse::from_raw(Some(&json!({"id": "resp_early_eof"}))),
            },
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "reasoning", "id": "rs_early_eof", "summary": []
                })),
            },
            ResponsesStreamEvent::ReasoningTextDelta {
                output_index: 0,
                delta: "partial reasoning before the stream ends".to_string(),
            },
        ];
        let mut processor =
            ResponsesStreamProcessor::new(&model, ResponsesStreamOptions::default());
        let (tx, mut rx) = mpsc::channel(64);
        for event in &events {
            processor.process_event(event, &tx).await.unwrap();
        }
        let error = processor.finish().unwrap_err();
        assert_eq!(
            error,
            "OpenAI Responses stream ended before a terminal response event"
        );
        // The partial thinking block was still emitted before the error.
        let mut events_out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events_out.push(event.event_type().to_string());
        }
        assert_eq!(events_out, ["thinking_start", "thinking_delta"]);
    }

    /// Upstream "tracks message phases" cases: `final_answer` messages
    /// provisionally stop the response; the terminal event settles the final
    /// reason.
    #[tokio::test]
    async fn message_phases_track_stop_reason() {
        let model = responses_model();
        let phased_events = |phase_added: &str,
                             phase_done: &str,
                             terminal: ResponsesStreamEvent| {
            vec![
                ResponsesStreamEvent::OutputItemAdded {
                    output_index: 0,
                    item: ResponsesOutputItem::from_value(json!({
                        "type": "message", "id": "msg_phase", "role": "assistant",
                        "status": "in_progress", "content": [], "phase": phase_added
                    })),
                },
                ResponsesStreamEvent::OutputItemDone {
                    output_index: 0,
                    item: ResponsesOutputItem::from_value(json!({
                        "type": "message", "id": "msg_phase", "role": "assistant",
                        "status": "completed",
                        "content": [{"type": "output_text", "text": "answer", "annotations": []}],
                        "phase": phase_done
                    })),
                },
                terminal,
            ]
        };
        let completed = || ResponsesStreamEvent::Completed {
            response: ResponsesResponse::from_raw(Some(&json!({
                "id": "resp_phase", "status": "completed"
            }))),
        };

        // commentary/commentary: pending until the terminal event.
        let (output, tracked) = run_events_tracked(
            &model,
            ResponsesStreamOptions::default(),
            phased_events("commentary", "commentary", completed()),
        )
        .await;
        assert_eq!(tracked[0], StopReason::Pending);
        assert_eq!(tracked[1], StopReason::Pending);
        assert_eq!(output.stop_reason, StopReason::Stop);

        // final_answer/final_answer: stopped from the added event on.
        let (output, tracked) = run_events_tracked(
            &model,
            ResponsesStreamOptions::default(),
            phased_events("final_answer", "final_answer", completed()),
        )
        .await;
        assert_eq!(tracked[0], StopReason::Stop);
        assert_eq!(tracked[1], StopReason::Stop);
        assert_eq!(output.stop_reason, StopReason::Stop);

        // commentary/final_answer: pending, then stop.
        let (_, tracked) = run_events_tracked(
            &model,
            ResponsesStreamOptions::default(),
            phased_events("commentary", "final_answer", completed()),
        )
        .await;
        assert_eq!(tracked[0], StopReason::Pending);
        assert_eq!(tracked[1], StopReason::Stop);

        // Upstream "replaces a provisional final-answer stop with an
        // incomplete terminal reason".
        let (output, tracked) = run_events_tracked(
            &model,
            ResponsesStreamOptions::default(),
            phased_events(
                "final_answer",
                "final_answer",
                ResponsesStreamEvent::Incomplete {
                    response: ResponsesResponse::from_raw(Some(&json!({
                        "id": "resp_phase", "status": "incomplete",
                        "incomplete_details": {"reason": "max_output_tokens"}
                    }))),
                },
            ),
        )
        .await;
        assert_eq!(tracked[0], StopReason::Stop);
        assert_eq!(tracked[1], StopReason::Stop);
        assert_eq!(output.stop_reason, StopReason::Length);
    }

    /// Upstream openai-responses-partial-json-cleanup.test.ts: streamed
    /// arguments parse live, and the persisted tool-call block carries no
    /// scratch buffer after output_item.done.
    #[tokio::test]
    async fn function_call_arguments_stream_and_cleanup() {
        let model = responses_model();
        let arguments_json = r#"{"path":"README.md","content":"updated"}"#;
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "function_call", "id": "fc_test",
                    "call_id": "call_test", "name": "edit", "arguments": ""
                })),
            },
            ResponsesStreamEvent::FunctionCallArgumentsDelta {
                output_index: 0,
                delta: r#"{"path":"README.md""#.to_string(),
            },
            ResponsesStreamEvent::FunctionCallArgumentsDelta {
                output_index: 0,
                delta: r#","content":"updated"}"#.to_string(),
            },
            ResponsesStreamEvent::FunctionCallArgumentsDone {
                output_index: 0,
                arguments: arguments_json.to_string(),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "function_call", "id": "fc_test",
                    "call_id": "call_test", "name": "edit", "arguments": arguments_json
                })),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test", "status": "completed"
                }))),
            },
        ];
        let (output, emitted) = run_events(&model, ResponsesStreamOptions::default(), events).await;

        assert_eq!(output.content.len(), 1);
        let persisted = match &output.content[0] {
            AssistantBlock::ToolCall(call) => call.clone(),
            other => panic!("expected toolCall block, got {other:?}"),
        };
        assert_eq!(
            persisted.arguments,
            json!({"path": "README.md", "content": "updated"})
        );
        // No partialJson scratch survives on the persisted block: the port
        // keeps scratch in the slot, structurally absent from the block.

        let deltas: Vec<&String> = emitted
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ToolcallDelta { delta, .. } => Some(delta),
                _ => None,
            })
            .collect();
        assert_eq!(
            deltas,
            [
                &r#"{"path":"README.md""#.to_string(),
                &r#","content":"updated"}"#.to_string()
            ],
            "done with identical arguments emits no extra delta"
        );
        let toolcall_end = emitted
            .iter()
            .find_map(|event| match event {
                AssistantMessageEvent::ToolcallEnd { tool_call, .. } => Some(tool_call),
                _ => None,
            })
            .expect("toolcall_end event");
        assert_eq!(toolcall_end, &persisted);
    }

    /// Live partial-JSON parsing: arguments are parsed after each delta
    /// (upstream assigns `parseStreamingJson(partialJson)` per delta).
    #[tokio::test]
    async fn function_call_arguments_parse_live() {
        let model = responses_model();
        let mut processor =
            ResponsesStreamProcessor::new(&model, ResponsesStreamOptions::default());
        let (tx, _rx) = mpsc::channel(64);
        processor
            .process_event(
                &ResponsesStreamEvent::OutputItemAdded {
                    output_index: 0,
                    item: ResponsesOutputItem::from_value(json!({
                        "type": "function_call", "id": "fc_test",
                        "call_id": "call_test", "name": "edit", "arguments": ""
                    })),
                },
                &tx,
            )
            .await
            .unwrap();
        processor
            .process_event(
                &ResponsesStreamEvent::FunctionCallArgumentsDelta {
                    output_index: 0,
                    delta: r#"{"path":"README.md""#.to_string(),
                },
                &tx,
            )
            .await
            .unwrap();
        match &processor.output().content[0] {
            AssistantBlock::ToolCall(call) => {
                assert_eq!(call.arguments, json!({"path": "README.md"}));
            }
            other => panic!("expected toolCall block, got {other:?}"),
        }
    }

    /// Upstream openai-responses-namespace.test.ts round-trip plus "omits an
    /// absent error message": the function_call done item's namespace lands
    /// on the tool call even without argument deltas, and completed streams
    /// with tool calls upgrade stop to toolUse.
    #[tokio::test]
    async fn completed_with_tool_calls_upgrades_to_tool_use() {
        let model = responses_model();
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "function_call", "id": "fc_test",
                    "call_id": "call_test", "name": "lookup", "arguments": ""
                })),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "function_call", "id": "fc_test",
                    "call_id": "call_test", "name": "lookup",
                    "arguments": "{\"value\":\"hello\"}", "namespace": "dynamic_tools"
                })),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test", "status": "completed"
                }))),
            },
        ];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;
        assert_eq!(output.stop_reason, StopReason::ToolUse);
        let tool_call = match &output.content[0] {
            AssistantBlock::ToolCall(call) => call,
            other => panic!("expected toolCall block, got {other:?}"),
        };
        assert_eq!(tool_call.id, "call_test|fc_test");
        assert_eq!(tool_call.namespace.as_deref(), Some("dynamic_tools"));
        // Upstream "omits an absent error message".
        assert!(output.error_message.is_none());
    }

    /// Custom tool-call input streams through the grammar JSON buffer: each
    /// delta emits the wrapped `{"input":"..."}` fragment, the done close
    /// finalizes, and a duplicate close emits nothing.
    #[tokio::test]
    async fn custom_tool_call_input_streams_through_grammar_buffer() {
        let model = responses_model();
        let mut options = ResponsesStreamOptions::default();
        options
            .grammar_tool_input_properties
            .insert("query".to_string(), "input".to_string());
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "custom_tool_call", "id": "ctc_test",
                    "call_id": "call_test", "name": "query", "input": ""
                })),
            },
            ResponsesStreamEvent::CustomToolCallInputDelta {
                output_index: 0,
                delta: "hel".to_string(),
            },
            ResponsesStreamEvent::CustomToolCallInputDelta {
                output_index: 0,
                delta: "lo".to_string(),
            },
            ResponsesStreamEvent::CustomToolCallInputDone {
                output_index: 0,
                input: "hello".to_string(),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "custom_tool_call", "id": "ctc_test",
                    "call_id": "call_test", "name": "query", "input": "hello"
                })),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test", "status": "completed"
                }))),
            },
        ];
        let (output, emitted) = run_events(&model, options, events).await;

        let deltas: Vec<String> = emitted
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ToolcallDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            deltas,
            [
                r#"{"input":"hel"#.to_string(),
                "lo".to_string(),
                r#""}"#.to_string()
            ],
            "done closes the grammar buffer with the final quote"
        );
        let tool_call = match &output.content[0] {
            AssistantBlock::ToolCall(call) => call,
            other => panic!("expected toolCall block, got {other:?}"),
        };
        assert_eq!(tool_call.id, "call_test|ctc_test");
        assert_eq!(tool_call.arguments, json!({"input": "hello"}));
    }

    /// Upstream output_item.done reasoning handling (lines 686-698): summary
    /// text wins over content text wins over the streamed deltas; the raw
    /// item is stored as the thinking signature.
    #[tokio::test]
    async fn reasoning_item_done_uses_summary_and_stores_signature() {
        let model = responses_model();
        let done_item = json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [
                {"type": "summary_text", "text": "S1"},
                {"type": "summary_text", "text": "S2"}
            ],
            "encrypted_content": "ENC"
        });
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "reasoning", "id": "rs_1", "summary": []
                })),
            },
            ResponsesStreamEvent::ReasoningSummaryTextDelta {
                output_index: 0,
                delta: "streamed".to_string(),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(done_item.clone()),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test", "status": "completed"
                }))),
            },
        ];
        let (output, emitted) = run_events(&model, ResponsesStreamOptions::default(), events).await;

        let thinking = match &output.content[0] {
            AssistantBlock::Thinking(thinking) => thinking,
            other => panic!("expected thinking block, got {other:?}"),
        };
        assert_eq!(thinking.thinking, "S1\n\nS2");
        assert_eq!(
            thinking.thinking_signature.as_deref(),
            Some(done_item.to_string().as_str())
        );
        let thinking_end = emitted
            .iter()
            .find_map(|event| match event {
                AssistantMessageEvent::ThinkingEnd { content, .. } => Some(content),
                _ => None,
            })
            .expect("thinking_end event");
        assert_eq!(thinking_end, "S1\n\nS2");
    }

    /// Upstream lines 687-689: without a summary the content array wins, and
    /// without either the streamed deltas are kept.
    #[tokio::test]
    async fn reasoning_done_falls_back_to_content_then_deltas() {
        let model = responses_model();
        let completed = || ResponsesStreamEvent::Completed {
            response: ResponsesResponse::from_raw(Some(&json!({
                "id": "resp_test", "status": "completed"
            }))),
        };

        // Content fallback.
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "reasoning", "id": "rs_1", "summary": []
                })),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "reasoning", "id": "rs_1", "summary": [],
                    "content": [{"type": "reasoning_text", "text": "C"}]
                })),
            },
            completed(),
        ];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;
        match &output.content[0] {
            AssistantBlock::Thinking(thinking) => assert_eq!(thinking.thinking, "C"),
            other => panic!("expected thinking block, got {other:?}"),
        }

        // Deltas kept.
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "reasoning", "id": "rs_1", "summary": []
                })),
            },
            ResponsesStreamEvent::ReasoningTextDelta {
                output_index: 0,
                delta: "streamed".to_string(),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "reasoning", "id": "rs_1", "summary": []
                })),
            },
            completed(),
        ];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;
        match &output.content[0] {
            AssistantBlock::Thinking(thinking) => assert_eq!(thinking.thinking, "streamed"),
            other => panic!("expected thinking block, got {other:?}"),
        }
    }

    /// Upstream reasoning_summary_part.done inserts the "\n\n" part
    /// separator into the thinking block.
    #[tokio::test]
    async fn reasoning_summary_part_done_inserts_separator() {
        let model = responses_model();
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "reasoning", "id": "rs_1", "summary": []
                })),
            },
            ResponsesStreamEvent::ReasoningSummaryTextDelta {
                output_index: 0,
                delta: "part one".to_string(),
            },
            ResponsesStreamEvent::ReasoningSummaryPartDone { output_index: 0 },
            ResponsesStreamEvent::ReasoningSummaryTextDelta {
                output_index: 0,
                delta: "part two".to_string(),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "reasoning", "id": "rs_1", "summary": []
                })),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test", "status": "completed"
                }))),
            },
        ];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;
        match &output.content[0] {
            AssistantBlock::Thinking(thinking) => {
                assert_eq!(thinking.thinking, "part one\n\npart two")
            }
            other => panic!("expected thinking block, got {other:?}"),
        }
    }

    /// Upstream lines 699-708: the message done item's content is
    /// authoritative and the text signature is the v1 JSON payload.
    #[tokio::test]
    async fn message_done_sets_text_and_signature() {
        let model = responses_model();
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "message", "id": "msg_1", "role": "assistant",
                    "status": "in_progress", "content": [], "phase": "commentary"
                })),
            },
            ResponsesStreamEvent::OutputTextDelta {
                output_index: 0,
                delta: "strea".to_string(),
            },
            ResponsesStreamEvent::OutputTextDelta {
                output_index: 0,
                delta: "med".to_string(),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "message", "id": "msg_1", "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": "final text", "annotations": []}],
                    "phase": "final_answer"
                })),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test", "status": "completed"
                }))),
            },
        ];
        let (output, emitted) = run_events(&model, ResponsesStreamOptions::default(), events).await;

        match &output.content[0] {
            AssistantBlock::Text(text) => {
                assert_eq!(text.text, "final text");
                assert_eq!(
                    text.text_signature.as_deref(),
                    Some(encode_text_signature_v1("msg_1", Some("final_answer")).as_str())
                );
            }
            other => panic!("expected text block, got {other:?}"),
        }
        let text_end = emitted
            .iter()
            .find_map(|event| match event {
                AssistantMessageEvent::TextEnd { content, .. } => Some(content),
                _ => None,
            })
            .expect("text_end event");
        assert_eq!(text_end, "final text");
    }

    /// Upstream lines 537-550 (pi issue #6409): the terminal response's
    /// output backfills encrypted_content into persisted reasoning
    /// signatures that lack it.
    #[tokio::test]
    async fn backfills_encrypted_content_from_terminal_response() {
        let model = responses_model();
        let done_item = json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": []
        });
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(done_item.clone()),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(done_item),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test",
                    "status": "completed",
                    "output": [
                        {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "ENC2"}
                    ]
                }))),
            },
        ];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;
        match &output.content[0] {
            AssistantBlock::Thinking(thinking) => {
                let signature: Value =
                    serde_json::from_str(thinking.thinking_signature.as_deref().unwrap()).unwrap();
                assert_eq!(signature["encrypted_content"], json!("ENC2"));
                assert_eq!(signature["id"], json!("rs_1"));
            }
            other => panic!("expected thinking block, got {other:?}"),
        }
    }

    /// Upstream lines 599-600: response.created records the response id;
    /// terminal responses can override it.
    #[tokio::test]
    async fn response_created_sets_response_id() {
        let model = responses_model();
        let events = vec![
            ResponsesStreamEvent::ResponseCreated {
                response: ResponsesResponse::from_raw(Some(&json!({"id": "resp_created"}))),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_final", "status": "completed"
                }))),
            },
        ];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;
        assert_eq!(output.response_id.as_deref(), Some("resp_final"));
    }

    /// Upstream lines 743-744: the top-level error event throws.
    #[tokio::test]
    async fn error_event_rejects_with_code_and_message() {
        let model = responses_model();
        let mut processor =
            ResponsesStreamProcessor::new(&model, ResponsesStreamOptions::default());
        let (tx, _rx) = mpsc::channel(64);
        let error = processor
            .process_event(
                &ResponsesStreamEvent::Error {
                    code: Some("server_error".to_string()),
                    message: Some("boom".to_string()),
                },
                &tx,
            )
            .await
            .unwrap_err();
        assert_eq!(error, "Error Code server_error: boom");
    }

    /// Upstream lines 643-652: refusal deltas append to the text block as
    /// text deltas, and the refusal content is authoritative at done.
    #[tokio::test]
    async fn refusal_delta_appends_to_text_block() {
        let model = responses_model();
        let events = vec![
            ResponsesStreamEvent::OutputItemAdded {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "message", "id": "msg_1", "role": "assistant",
                    "status": "in_progress", "content": []
                })),
            },
            ResponsesStreamEvent::RefusalDelta {
                output_index: 0,
                delta: "I cannot".to_string(),
            },
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "message", "id": "msg_1", "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "refusal", "refusal": "I cannot help with that"}]
                })),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test", "status": "completed"
                }))),
            },
        ];
        let (output, emitted) = run_events(&model, ResponsesStreamOptions::default(), events).await;
        match &output.content[0] {
            AssistantBlock::Text(text) => assert_eq!(text.text, "I cannot help with that"),
            other => panic!("expected text block, got {other:?}"),
        }
        assert!(emitted.iter().any(|event| matches!(
            event,
            AssistantMessageEvent::TextDelta { delta, .. } if delta == "I cannot"
        )));
    }

    /// Upstream getOrCreateSlot (line 684): an output_item.done without a
    /// preceding added event creates the slot (emitting the start event) and
    /// finalizes it.
    #[tokio::test]
    async fn late_output_item_done_creates_and_finalizes_slot() {
        let model = responses_model();
        let events = vec![
            ResponsesStreamEvent::OutputItemDone {
                output_index: 0,
                item: ResponsesOutputItem::from_value(json!({
                    "type": "function_call", "id": "fc_test",
                    "call_id": "call_test", "name": "edit",
                    "arguments": "{\"a\":1}"
                })),
            },
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_test", "status": "completed"
                }))),
            },
        ];
        let (output, emitted) = run_events(&model, ResponsesStreamOptions::default(), events).await;
        let kinds: Vec<&str> = emitted.iter().map(|event| event.event_type()).collect();
        assert_eq!(kinds, ["toolcall_start", "toolcall_end"]);
        match &output.content[0] {
            AssistantBlock::ToolCall(call) => assert_eq!(call.arguments, json!({"a": 1})),
            other => panic!("expected toolCall block, got {other:?}"),
        }
    }

    /// Upstream getSlot type checks: deltas for a different slot type at the
    /// same output index are ignored, and unknown output item types create
    /// no slot.
    #[tokio::test]
    async fn type_mismatched_deltas_are_ignored() {
        let model = responses_model();
        let mut processor =
            ResponsesStreamProcessor::new(&model, ResponsesStreamOptions::default());
        let (tx, mut rx) = mpsc::channel(64);
        // A function_call slot exists at index 0.
        processor
            .process_event(
                &ResponsesStreamEvent::OutputItemAdded {
                    output_index: 0,
                    item: ResponsesOutputItem::from_value(json!({
                        "type": "function_call", "id": "fc_test",
                        "call_id": "call_test", "name": "edit", "arguments": ""
                    })),
                },
                &tx,
            )
            .await
            .unwrap();
        // Text deltas target the toolCall slot: ignored.
        processor
            .process_event(
                &ResponsesStreamEvent::OutputTextDelta {
                    output_index: 0,
                    delta: "nope".to_string(),
                },
                &tx,
            )
            .await
            .unwrap();
        // Unknown item types create nothing.
        processor
            .process_event(
                &ResponsesStreamEvent::OutputItemAdded {
                    output_index: 1,
                    item: ResponsesOutputItem::from_value(json!({
                        "type": "web_search_call", "id": "ws_1"
                    })),
                },
                &tx,
            )
            .await
            .unwrap();
        // Unhandled event types are ignored entirely.
        processor
            .process_event(&ResponsesStreamEvent::Unhandled, &tx)
            .await
            .unwrap();
        // Terminal event (no additional pushed events).
        processor
            .process_event(
                &ResponsesStreamEvent::Completed {
                    response: ResponsesResponse::from_raw(Some(&json!({
                        "id": "resp_test", "status": "completed"
                    }))),
                },
                &tx,
            )
            .await
            .unwrap();
        processor.finish().unwrap();
        drop(tx);
        let mut emitted = Vec::new();
        while let Ok(event) = rx.try_recv() {
            emitted.push(event.event_type().to_string());
        }
        assert_eq!(emitted, ["toolcall_start"]);
        assert_eq!(processor.output().content.len(), 1);
    }

    /// Upstream lines 565-575: reasoning tokens are captured from
    /// output_tokens_details.
    #[tokio::test]
    async fn usage_reasoning_tokens_captured() {
        let model = responses_model();
        let events = vec![ResponsesStreamEvent::Completed {
            response: ResponsesResponse::from_raw(Some(&json!({
                "id": "resp_test",
                "status": "completed",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 8,
                    "total_tokens": 18,
                    "output_tokens_details": {"reasoning_tokens": 5}
                }
            }))),
        }];
        let (output, _) = run_events(&model, ResponsesStreamOptions::default(), events).await;
        assert_eq!(output.usage.reasoning, Some(5));
        assert_eq!(output.usage.output, 8);
    }

    /// The T8 seam: SSE `data:` frames parse straight into typed events,
    /// unknown event types included.
    #[test]
    fn stream_events_parse_from_sse_frames() {
        let completed: ResponsesStreamEvent = serde_json::from_str(
            r#"{"type":"response.completed","sequence_number":0,"response":{"id":"resp_completed","status":"completed","usage":{"input_tokens":20,"output_tokens":7,"total_tokens":27,"input_tokens_details":{"cached_tokens":2,"cache_write_tokens":3}}}}"#,
        )
        .unwrap();
        assert_eq!(
            completed,
            ResponsesStreamEvent::Completed {
                response: ResponsesResponse::from_raw(Some(&json!({
                    "id": "resp_completed",
                    "status": "completed",
                    "usage": {
                        "input_tokens": 20,
                        "output_tokens": 7,
                        "total_tokens": 27,
                        "input_tokens_details": {"cached_tokens": 2, "cache_write_tokens": 3}
                    }
                })))
            }
        );

        let added: ResponsesStreamEvent = serde_json::from_str(
            r#"{"type":"response.output_item.added","sequence_number":0,"output_index":0,"item":{"type":"function_call","id":"fc_test","call_id":"call_test","name":"lookup","arguments":""}}"#,
        )
        .unwrap();
        match added {
            ResponsesStreamEvent::OutputItemAdded { output_index, item } => {
                assert_eq!(output_index, 0);
                match item {
                    ResponsesOutputItem::FunctionCall(call) => {
                        assert_eq!(call.id, "fc_test");
                        assert_eq!(call.call_id, "call_test");
                        assert_eq!(call.name, "lookup");
                        assert_eq!(call.arguments, "");
                    }
                    other => panic!("expected function_call item, got {other:?}"),
                }
            }
            other => panic!("expected output_item.added, got {other:?}"),
        }

        // Unknown event types parse as unhandled instead of failing.
        let unknown: ResponsesStreamEvent = serde_json::from_str(
            r#"{"type":"response.output_text.done","output_index":0,"text":"x"}"#,
        )
        .unwrap();
        assert_eq!(unknown, ResponsesStreamEvent::Unhandled);
    }

    /// Upstream openai-responses.ts createClient session-affinity block
    /// (lines 258-267) + cache-affinity-e2e session id format.
    #[test]
    fn session_affinity_header_formats() {
        let mut model = responses_model();

        // OpenAI format (default): session_id + x-client-request-id.
        assert_eq!(
            session_affinity_headers(SessionAffinityFormat::Openai, "0195d6e4"),
            [
                ("session_id".to_string(), "0195d6e4".to_string()),
                ("x-client-request-id".to_string(), "0195d6e4".to_string())
            ]
        );

        // OpenRouter format: x-session-id.
        assert_eq!(
            session_affinity_headers(SessionAffinityFormat::Openrouter, "0195d6e4"),
            [("x-session-id".to_string(), "0195d6e4".to_string())]
        );

        // openai-nosession: x-client-request-id only.
        assert_eq!(
            session_affinity_headers(SessionAffinityFormat::OpenaiNosession, "0195d6e4"),
            [("x-client-request-id".to_string(), "0195d6e4".to_string())]
        );

        // Detection: provider or base URL.
        assert_eq!(
            detect_session_affinity_format(&model),
            SessionAffinityFormat::Openai
        );
        model.provider = "openrouter".to_string();
        assert_eq!(
            detect_session_affinity_format(&model),
            SessionAffinityFormat::Openrouter
        );
        model.provider = "openai".to_string();
        model.base_url = "https://openrouter.ai/api/v1".to_string();
        assert_eq!(
            detect_session_affinity_format(&model),
            SessionAffinityFormat::Openrouter
        );
    }
}
