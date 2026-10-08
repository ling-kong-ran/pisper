//! Tests for [`super`] — the port of upstream `coding-agent` compaction.
//!
//! Two layers, per the slice standard:
//! 1. The upstream executable spec (`test/compaction.test.ts`,
//!    `test/branch-summarization.test.ts`,
//!    `test/compaction-summary-reasoning.test.ts`,
//!    `test/compaction-serialization.test.ts`) ported assertion-for-assertion
//!    against scripted [`super::StreamFn`] transports.
//! 2. Oracle byte-comparisons against
//!    `tests/fixtures/core_oracle_compaction/compaction.oracle.json`, captured from
//!    the real upstream TypeScript under node (see the generator script next
//!    to the capture). Structured values compare as canonical JSON
//!    (recursively key-sorted on both sides); timestamps and abort signals are
//!    scrubbed on capture; fresh uuidv7 routing ids are normalized to their
//!    shape.
//!
//! Upstream test blocks that depend on modules outside this slice are
//! disclosed, not ported: `buildSessionContext` behavior lives in the
//! session-manager slice (its `messages` projection is exercised here through
//! the vendored seam builders); the `Large session fixture` block needs
//! `parseSessionEntries`/`migrateSessionEntries` plus the jsonl fixture; the
//! `skipIf(!ANTHROPIC_OAUTH_TOKEN)` block makes live API calls (upstream skips
//! it without a key, too); `agent-session-compaction.test.ts`,
//! `compaction-extensions.test.ts`, `settings-manager-compaction.test.ts`,
//! `interactive-mode-compaction.test.ts`, `trigger-compact-extension.test.ts`,
//! `branch-summary-extensions.test.ts`,
//! `agent-session-auto-compaction-queue.test.ts` and
//! `agent-session-branching.test.ts` drive `AgentSession`/extension/settings
//! seams above this layer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::*;
use crate::ai::models::faux::{
    faux_provider, FauxModelDefinition, FauxProviderOptions, FauxResponseStep,
};
use crate::ai::models::{create_models, CreateModelsOptions};
use crate::ai::types::content::{ThinkingContent, ToolCall};
use crate::ai::types::primitives::ToolChoice;

// ---------------------------------------------------------------------------
// Oracle plumbing
// ---------------------------------------------------------------------------

const ORACLE: &str =
    include_str!("../../../tests/fixtures/core_oracle_compaction/compaction.oracle.json");

/// The capture is a static JSON document; parsed fresh per test.
fn oracle() -> Value {
    serde_json::from_str(ORACLE).expect("oracle json parses")
}

/// Canonical JSON: recursive key sort (mirrors the oracle's capture
/// normalization; correct regardless of serde_json feature flags).
fn canon(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for key in keys {
                let child = map.get(&key).cloned().unwrap_or(Value::Null);
                out.insert(key, canon(&child));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canon).collect()),
        other => other.clone(),
    }
}

fn canon_string(value: &Value) -> String {
    canon(value).to_string()
}

/// Replace every message `timestamp` with the oracle's `"<ts>"` marker.
fn scrub_timestamps(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.contains_key("timestamp") {
                map.insert("timestamp".to_string(), json!("<ts>"));
            }
            for (_key, child) in map.iter_mut() {
                scrub_timestamps(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                scrub_timestamps(child);
            }
        }
        _ => {}
    }
}

/// Caller-set session ids survive verbatim; fresh uuidv7 routing ids
/// normalize to a shape marker (they are nondeterministic).
const DETERMINISTIC_SESSION_IDS: [&str; 2] = ["route-1", "current-routing-session"];

fn is_uuid_v7(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[14] == b'7'
        && bytes[18] == b'-'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
        && bytes[23] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 19 || matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

fn scrub_options(options: &SimpleStreamOptions) -> Value {
    let mut value = serde_json::to_value(options).expect("options serialize");
    if let Value::Object(map) = &mut value {
        map.shift_remove("signal");
        let session_id = map
            .get("sessionId")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        if let Some(session_id) = session_id {
            if !DETERMINISTIC_SESSION_IDS.contains(&session_id.as_str()) {
                let marker = if is_uuid_v7(&session_id) {
                    "<uuid-v7>"
                } else {
                    "<unexpected-session-id>"
                };
                map.insert("sessionId".to_string(), json!(marker));
            }
        }
    }
    scrub_timestamps(&mut value);
    value
}

fn scrub_context(context: &TranscriptContext) -> Value {
    let mut value = json!({ "messages": context.messages() });
    scrub_timestamps(&mut value);
    value
}

#[derive(Clone)]
struct CapturedCall {
    context: Value,
    options: Value,
}

type CallLog = Arc<Mutex<Vec<CapturedCall>>>;

/// Scripted `StreamFn`: records every `(context, options)` pair and replays
/// the given responses in order (upstream tests mock `completeSimple` or pass
/// a scripted `streamFn`; both play the same role here).
fn scripted_stream_fn(responses: Vec<AssistantMessage>, calls: &CallLog) -> Option<StreamFn> {
    let responses = Arc::new(responses);
    let calls = Arc::clone(calls);
    Some(Arc::new(move |_model, context, options| {
        let response = {
            let mut log = calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let response = responses
                .get(log.len())
                .cloned()
                .unwrap_or_else(|| panic!("no scripted response for call {}", log.len()));
            log.push(CapturedCall {
                context: scrub_context(&context),
                options: scrub_options(&options),
            });
            response
        };
        Box::pin(async move { response })
    }))
}

fn calls_of(calls: &CallLog) -> Vec<(Value, Value)> {
    calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|call| (call.context.clone(), call.options.clone()))
        .collect()
}

fn options_json_of(value: &Value) -> String {
    canon_string(value)
}

fn context_json(value: &Value) -> String {
    canon_string(value)
}

fn usage_json(usage: &Usage) -> String {
    canon_string(&serde_json::to_value(usage).expect("usage serializes"))
}

fn file_ops_json(file_ops: &FileOperations) -> String {
    let mut read: Vec<String> = file_ops.read.iter().cloned().collect();
    let mut written: Vec<String> = file_ops.written.iter().cloned().collect();
    let mut edited: Vec<String> = file_ops.edited.iter().cloned().collect();
    read.sort();
    written.sort();
    edited.sort();
    canon_string(&json!({ "read": read, "written": written, "edited": edited }))
}

fn roles_of(messages: &[AgentMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| json!(message.role()))
        .collect()
}

/// Upstream test helper `extractText` (compaction.test.ts:153-187).
fn extract_text(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .map(|message| match message {
            AgentMessage::User(user) => match &user.content {
                StringOrBlocks::Text(text) => text.clone(),
                StringOrBlocks::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|block| match block {
                        TextOrImageBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<&str>>()
                    .join(" "),
            },
            AgentMessage::Assistant(assistant) => assistant_blocks_text(&assistant.content, " "),
            AgentMessage::Custom(custom) => match custom.role.as_str() {
                "branchSummary" | "compactionSummary" => custom
                    .data
                    .get("summary")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
                _ => String::new(),
            },
            _ => String::new(),
        })
        .collect::<Vec<String>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Fixtures (upstream test heads)
// ---------------------------------------------------------------------------

const TS: i64 = 1_767_337_445_678;
const STAMP: &str = "2026-01-02T03:04:05.000Z";

fn mock_usage(input: u64, output: u64) -> Usage {
    mock_usage_full(input, output, 0, 0)
}

fn mock_usage_full(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output + cache_read + cache_write,
        cost: UsageCost::default(),
    }
}

fn user_message(text: &str) -> AgentMessage {
    user_message_at(text, TS)
}

fn user_message_at(text: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp,
    })
}

fn text_block(text: &str) -> AssistantBlock {
    AssistantBlock::Text(TextContent {
        text: text.to_string(),
        text_signature: None,
    })
}

fn tool_call_block(name: &str, arguments: Value) -> AssistantBlock {
    AssistantBlock::ToolCall(ToolCall {
        id: format!("call-{name}-{}", arguments.to_string().len()),
        name: name.to_string(),
        arguments,
        thought_signature: None,
        namespace: None,
    })
}

fn assistant_message(text: &str) -> AssistantMessage {
    assistant_message_with_usage(text, mock_usage(100, 50))
}

fn assistant_message_with_usage(text: &str, usage: Usage) -> AssistantMessage {
    AssistantMessage {
        content: vec![text_block(text)],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        usage,
        stop_reason: StopReason::Stop,
        timestamp: TS,
        ..text_response("unused")
    }
}

/// `textResponse` (oracle fixture): a scripted summary response.
fn text_response(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![text_block(text)],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "test-model".to_string(),
        usage: mock_usage(10, 10),
        stop_reason: StopReason::Stop,
        timestamp: TS,
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
    }
}

fn text_response_with_usage(text: &str, usage: Usage) -> AssistantMessage {
    AssistantMessage {
        usage,
        ..text_response(text)
    }
}

fn tool_call_response() -> AssistantMessage {
    AssistantMessage {
        content: vec![tool_call_block("read", json!({"path": "README.md"}))],
        stop_reason: StopReason::ToolUse,
        ..text_response("x")
    }
}

fn test_model() -> Model {
    test_model_with(false, 8192)
}

fn test_model_with(reasoning: bool, max_tokens: u64) -> Model {
    serde_json::from_value(json!({
        "id": if reasoning { "reasoning-model" } else { "test-model" },
        "name": "Test Model",
        "api": "anthropic-messages",
        "provider": "anthropic",
        "baseUrl": "https://api.anthropic.com",
        "reasoning": reasoning,
        "input": ["text"],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 200000,
        "maxTokens": max_tokens
    }))
    .expect("model fixture parses")
}

/// Session-entry builder mirroring the oracle fixture ids (`t-0`, `t-1`, ...)
/// and the upstream per-test `beforeEach` reset.
#[derive(Default)]
struct Fixture {
    counter: usize,
    last_id: Option<String>,
}

impl Fixture {
    fn reset(&mut self) {
        *self = Fixture::default();
    }

    fn base_entry(&mut self) -> (String, Option<String>) {
        let id = format!("t-{}", self.counter);
        self.counter += 1;
        let parent_id = self.last_id.clone();
        self.last_id = Some(id.clone());
        (id, parent_id)
    }

    fn message_entry(&mut self, message: AgentMessage) -> SessionEntry {
        let (id, parent_id) = self.base_entry();
        SessionEntry::Message {
            id,
            parent_id,
            message,
        }
    }

    fn compaction_entry(&mut self, summary: &str, first_kept_entry_id: &str) -> SessionEntry {
        self.compaction_entry_with(summary, first_kept_entry_id, None, false)
    }

    fn compaction_entry_with(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        details: Option<Value>,
        from_hook: bool,
    ) -> SessionEntry {
        let (id, parent_id) = self.base_entry();
        SessionEntry::Compaction {
            id,
            parent_id,
            summary: summary.to_string(),
            first_kept_entry_id: first_kept_entry_id.to_string(),
            tokens_before: 10_000,
            details,
            from_hook,
            system_message: None,
            timestamp: STAMP.to_string(),
        }
    }

    fn custom_message_entry(&mut self, content: &str) -> SessionEntry {
        let (id, parent_id) = self.base_entry();
        SessionEntry::CustomMessage {
            id,
            parent_id,
            custom_type: "test".to_string(),
            content: CustomMessageContent::Text(content.to_string()),
            display: true,
            details: None,
            timestamp: STAMP.to_string(),
        }
    }

    fn tool_result_entry(&mut self, text: &str) -> SessionEntry {
        let (id, parent_id) = self.base_entry();
        SessionEntry::Message {
            id,
            parent_id,
            message: AgentMessage::ToolResult(crate::ai::types::ToolResultMessage {
                tool_call_id: "tc1".to_string(),
                tool_name: "read".to_string(),
                content: vec![TextOrImageBlock::Text(TextContent {
                    text: text.to_string(),
                    text_signature: None,
                })],
                details: None,
                usage: None,
                is_error: false,
                timestamp: TS,
            }),
        }
    }
}

fn filler(text: &str, size: usize) -> String {
    let mut text = text.to_string();
    let len = text.chars().count();
    if size > len {
        text.extend(std::iter::repeat_n(' ', size - len));
    }
    text
}

fn default_settings() -> CompactionSettings {
    CompactionSettings {
        enabled: true,
        reserve_tokens: 2_000,
        keep_recent_tokens: 20,
    }
}

fn summary_request<'a>(
    messages: &'a [AgentMessage],
    model: &'a Model,
    reserve_tokens: u64,
    api_key: Option<&'a str>,
    stream_fn: Option<StreamFn>,
    callbacks: &'a mut RetryCallbacks,
) -> SummaryRequest<'a> {
    SummaryRequest {
        messages,
        model,
        reserve_tokens,
        api_key,
        headers: None,
        signal: None,
        custom_instructions: None,
        previous_summary: None,
        thinking_level: None,
        stream_fn,
        env: None,
        retry: None,
        callbacks,
        session_id: None,
    }
}

fn compact_options<'a>(
    model: &'a Model,
    stream_fn: Option<StreamFn>,
    callbacks: &'a mut RetryCallbacks,
) -> CompactOptions<'a> {
    CompactOptions {
        model,
        api_key: Some("test-key"),
        headers: None,
        custom_instructions: None,
        signal: None,
        thinking_level: None,
        stream_fn,
        env: None,
        retry: None,
        callbacks,
        session_id: None,
    }
}

fn branch_options<'a>(
    model: &'a Model,
    stream_fn: Option<StreamFn>,
    callbacks: &'a mut RetryCallbacks,
) -> GenerateBranchSummaryOptions<'a> {
    GenerateBranchSummaryOptions {
        model,
        api_key: Some("test-key"),
        headers: None,
        env: None,
        signal: None,
        custom_instructions: None,
        replace_instructions: false,
        reserve_tokens: None,
        stream_fn,
        retry: None,
        callbacks,
    }
}

fn split_preparation(
    messages_to_summarize: Vec<AgentMessage>,
    previous_summary: Option<&str>,
) -> CompactionPreparation {
    CompactionPreparation {
        first_kept_entry_id: "entry-keep".to_string(),
        messages_to_summarize,
        turn_prefix_messages: vec![user_message("Summarize this.")],
        is_split_turn: true,
        tokens_before: 100,
        previous_summary: previous_summary.map(str::to_string),
        file_ops: FileOperations::new(),
        settings: default_settings(),
    }
}

/// Turn-start helper matching the oracle's `findCutPoint` capture shape.
fn cut_json(result: CutPointResult) -> Value {
    json!({
        "firstKeptEntryIndex": result.first_kept_entry_index,
        "turnStartIndex": result.turn_start_index.map_or(-1, |index| index as i64),
        "isSplitTurn": result.is_split_turn,
    })
}

// ---------------------------------------------------------------------------
// Ported spec: token calculation (compaction.test.ts "Token calculation")
// ---------------------------------------------------------------------------

#[test]
fn calculates_total_context_tokens_from_usage() {
    assert_eq!(
        calculate_context_tokens(mock_usage_full(1000, 500, 200, 100)),
        1800
    );
}

#[test]
fn handles_zero_values() {
    assert_eq!(calculate_context_tokens(mock_usage(0, 0)), 0);
}

// ---------------------------------------------------------------------------
// Ported spec: getLastAssistantUsage (compaction.test.ts)
// ---------------------------------------------------------------------------

#[test]
fn finds_the_last_non_aborted_assistant_message_usage() {
    let mut fixture = Fixture::default();
    let entries = vec![
        fixture.message_entry(user_message("Hello")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Hi",
            mock_usage(100, 50),
        ))),
        fixture.message_entry(user_message("How are you?")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Good",
            mock_usage(200, 100),
        ))),
    ];

    let usage = get_last_assistant_usage(&entries).expect("usage");
    assert_eq!(usage.input, 200);
}

#[test]
fn skips_aborted_messages() {
    let mut fixture = Fixture::default();
    let mut aborted = assistant_message_with_usage("Aborted", mock_usage(300, 150));
    aborted.stop_reason = StopReason::Aborted;
    let entries = vec![
        fixture.message_entry(user_message("Hello")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Hi",
            mock_usage(100, 50),
        ))),
        fixture.message_entry(user_message("How are you?")),
        fixture.message_entry(AgentMessage::Assistant(aborted)),
    ];

    let usage = get_last_assistant_usage(&entries).expect("usage");
    assert_eq!(usage.input, 100);
}

#[test]
fn skips_all_zero_assistant_usage() {
    let mut fixture = Fixture::default();
    let entries = vec![
        fixture.message_entry(user_message("Hello")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Hi",
            mock_usage(100, 50),
        ))),
        fixture.message_entry(user_message("continue")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Partial",
            mock_usage(0, 0),
        ))),
    ];

    let usage = get_last_assistant_usage(&entries).expect("usage");
    assert_eq!(usage.input, 100);
}

#[test]
fn returns_none_if_no_assistant_messages() {
    let mut fixture = Fixture::default();
    let entries = vec![fixture.message_entry(user_message("Hello"))];
    assert!(get_last_assistant_usage(&entries).is_none());
}

#[test]
fn skips_error_stop_messages() {
    let mut fixture = Fixture::default();
    let mut failed = assistant_message_with_usage("Err", mock_usage(500, 50));
    failed.stop_reason = StopReason::Error;
    failed.error_message = Some("boom".to_string());
    let entries = vec![
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Bad",
            mock_usage(400, 40),
        ))),
        fixture.message_entry(AgentMessage::Assistant(failed)),
    ];

    let usage = get_last_assistant_usage(&entries).expect("usage");
    assert_eq!(usage.input, 400);
}

// ---------------------------------------------------------------------------
// Ported spec: estimateContextTokens / shouldCompact (compaction.test.ts)
// ---------------------------------------------------------------------------

#[test]
fn uses_the_last_non_zero_assistant_usage_as_the_context_anchor() {
    let messages = vec![
        user_message("Hello"),
        AgentMessage::Assistant(assistant_message_with_usage("Hi", mock_usage(100, 50))),
        user_message("continue"),
        AgentMessage::Assistant(assistant_message_with_usage(
            "Partial thinking",
            mock_usage(0, 0),
        )),
    ];

    let estimate = estimate_context_tokens(&messages);

    assert_eq!(estimate.usage_tokens, 150);
    assert_eq!(estimate.last_usage_index, Some(1));
    assert!(estimate.trailing_tokens > 0);
    assert_eq!(estimate.tokens, 150 + estimate.trailing_tokens);
}

#[test]
fn returns_true_when_context_exceeds_threshold() {
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 10_000,
        keep_recent_tokens: 20_000,
    };

    assert!(should_compact(95_000, 100_000, settings));
    assert!(!should_compact(89_000, 100_000, settings));
}

#[test]
fn returns_false_when_disabled() {
    let settings = CompactionSettings {
        enabled: false,
        reserve_tokens: 10_000,
        keep_recent_tokens: 20_000,
    };

    assert!(!should_compact(95_000, 100_000, settings));
}

// ---------------------------------------------------------------------------
// Ported spec: findCutPoint (compaction.test.ts)
// ---------------------------------------------------------------------------

/// Char-heavy messages so the keepRecentTokens budget crosses (the heuristic
/// counts chars/4; the provider usage in the fixtures is irrelevant, matching
/// upstream).
fn cut_point_fixture() -> Vec<Vec<SessionEntry>> {
    let mut fixture = Fixture::default();
    let mut token_diff = Vec::new();
    for i in 0..10 {
        token_diff.push(fixture.message_entry(user_message(&filler(&format!("User {i}"), 800))));
        token_diff.push(fixture.message_entry(AgentMessage::Assistant(
            assistant_message_with_usage(
                &filler(&format!("Assistant {i}"), 800),
                mock_usage_full(0, 100, (i + 1) * 1000, 0),
            ),
        )));
    }
    fixture.reset();
    let single_assistant =
        vec![
            fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
                "a",
                mock_usage(0, 0),
            ))),
        ];
    fixture.reset();
    let all_fit = vec![
        fixture.message_entry(user_message(&filler("1", 400))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &filler("a", 400),
            mock_usage_full(0, 50, 500, 0),
        ))),
        fixture.message_entry(user_message(&filler("2", 400))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &filler("b", 400),
            mock_usage_full(0, 50, 1000, 0),
        ))),
    ];
    fixture.reset();
    let split_turn = vec![
        fixture.message_entry(user_message(&filler("Turn 1", 400))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &filler("A1", 400),
            mock_usage_full(0, 100, 1000, 0),
        ))),
        fixture.message_entry(user_message(&filler("Turn 2", 400))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &filler("A2-1", 400),
            mock_usage_full(0, 100, 5000, 0),
        ))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &filler("A2-2", 400),
            mock_usage_full(0, 100, 8000, 0),
        ))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &filler("A2-3", 400),
            mock_usage_full(0, 100, 10000, 0),
        ))),
    ];
    fixture.reset();
    let custom_budget = vec![
        fixture.message_entry(user_message("hi")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message("hello"))),
        fixture.custom_message_entry(&"x".repeat(4000)),
        fixture.message_entry(AgentMessage::Assistant(assistant_message("ok"))),
    ];
    // Cut lands past a metadata run; the back-scan must slide the cut index
    // back over the metadata (no context messages) and stop at the tool
    // result.
    fixture.reset();
    let metadata_scan = vec![
        fixture.message_entry(user_message(&filler("u1", 1000))),
        fixture.tool_result_entry(&filler("r", 1000)),
        {
            // model_change entry (metadata, no context messages)
            let _ = fixture.base_entry();
            SessionEntry::Other
        },
        {
            // label entry (metadata, no context messages)
            let _ = fixture.base_entry();
            SessionEntry::Other
        },
        fixture.message_entry(user_message(&filler("u2", 1000))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &filler("a2", 1000),
            mock_usage_full(0, 100, 6000, 0),
        ))),
    ];
    fixture.reset();
    let tool_results_never_cut = vec![
        fixture.message_entry(user_message(&filler("u1", 1000))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &filler("a1", 1000),
            mock_usage_full(0, 50, 4000, 0),
        ))),
        fixture.tool_result_entry(&"x".repeat(9000)),
        fixture.message_entry(user_message(&filler("u2", 1000))),
    ];
    vec![
        token_diff,
        single_assistant,
        all_fit,
        split_turn,
        custom_budget,
        metadata_scan,
        tool_results_never_cut,
    ]
}

#[test]
fn finds_a_cut_point_based_on_token_differences() {
    let batteries = cut_point_fixture();
    let token_diff = &batteries[0];

    let result = find_cut_point(token_diff, 0, token_diff.len(), 2500);
    assert!(matches!(
        &token_diff[result.first_kept_entry_index],
        SessionEntry::Message { message, .. }
            if matches!(message.role(), "user" | "assistant")
    ));
}

#[test]
fn returns_start_index_if_no_valid_cut_points_in_range() {
    let batteries = cut_point_fixture();
    let single_assistant = &batteries[1];

    let result = find_cut_point(single_assistant, 0, single_assistant.len(), 1000);
    assert_eq!(result.first_kept_entry_index, 0);
}

#[test]
fn keeps_everything_if_all_messages_fit_within_budget() {
    let batteries = cut_point_fixture();
    let all_fit = &batteries[2];

    let result = find_cut_point(all_fit, 0, all_fit.len(), 50_000);
    assert_eq!(result.first_kept_entry_index, 0);
}

#[test]
fn indicates_split_turn_when_cutting_at_assistant_message() {
    let batteries = cut_point_fixture();
    let split_turn = &batteries[3];

    let result = find_cut_point(split_turn, 0, split_turn.len(), 250);
    let cut_entry = &split_turn[result.first_kept_entry_index];
    if matches!(cut_entry, SessionEntry::Message { message, .. } if message.role() == "assistant") {
        assert!(result.is_split_turn);
        assert_eq!(result.turn_start_index, Some(2));
    } else {
        // Upstream tolerates the user-message cut; the oracle pins the exact
        // index either way.
        assert!(matches!(
            cut_entry,
            SessionEntry::Message { message, .. } if message.role() == "user"
        ));
    }
}

#[test]
fn budgets_context_visible_custom_message_entries() {
    let batteries = cut_point_fixture();
    let custom_budget = &batteries[4];

    let tiny_budget = find_cut_point(custom_budget, 0, custom_budget.len(), 1);
    assert_eq!(tiny_budget.first_kept_entry_index, 3);
    assert!(tiny_budget.is_split_turn);
    assert_eq!(tiny_budget.turn_start_index, Some(2));

    let custom_fits = find_cut_point(custom_budget, 0, custom_budget.len(), 2);
    assert_eq!(custom_fits.first_kept_entry_index, 2);
    assert!(!custom_fits.is_split_turn);
    assert_eq!(custom_fits.turn_start_index, None);
}

// ---------------------------------------------------------------------------
// Ported spec: prepareCompaction (compaction.test.ts)
// ---------------------------------------------------------------------------

#[test]
fn does_not_treat_system_messages_as_conversation_history() {
    let mut fixture = Fixture::default();
    let system_message = AgentMessage::System(crate::ai::types::SystemMessage {
        content: StringOrBlocks::Text(String::new()),
        sections: Some(crate::ai::types::Sections::new(vec![(
            "preamble".to_string(),
            Some("current prompt".to_string()),
        )])),
        tools_added: None,
        tools_removed: None,
        timestamp: TS,
    });
    let turn_message = user_message("one long turn");
    let system_entry = fixture.message_entry(system_message);
    let user_entry = fixture.message_entry(turn_message.clone());
    let assistant_entry = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        "assistant suffix",
    )));
    let assistant_id = assistant_entry.id().unwrap().to_string();

    let preparation = prepare_compaction(
        &[system_entry, user_entry, assistant_entry],
        CompactionSettings {
            keep_recent_tokens: 1,
            ..DEFAULT_COMPACTION_SETTINGS
        },
    )
    .expect("preparation");

    assert_eq!(preparation.first_kept_entry_id, assistant_id);
    assert!(preparation.is_split_turn);
    assert!(preparation.messages_to_summarize.is_empty());
    assert_eq!(preparation.turn_prefix_messages, vec![turn_message]);
}

#[test]
fn skips_repeated_compactions_when_kept_messages_still_fit() {
    let mut fixture = Fixture::default();
    let u1 = fixture.message_entry(user_message("user msg 1 (summarized by compaction1)"));
    let a1 = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        "assistant msg 1",
    )));
    let u2 = fixture.message_entry(user_message("user msg 2 - kept by compaction1"));
    let a2 = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        "assistant msg 2",
    )));
    let u3 = fixture.message_entry(user_message("user msg 3 - kept by compaction1"));
    let a3 = fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
        "assistant msg 3",
        mock_usage(5000, 1000),
    )));
    let compaction1 = fixture.compaction_entry("First summary", u2.id().unwrap());
    let u4 = fixture.message_entry(user_message("user msg 4 (new after compaction1)"));
    let a4 = fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
        "assistant msg 4",
        mock_usage(8000, 2000),
    )));

    let path_entries = [u1, a1, u2, a2, u3, a3, compaction1, u4, a4];
    assert!(prepare_compaction(&path_entries, DEFAULT_COMPACTION_SETTINGS).is_none());
}

#[test]
fn re_summarizes_previously_kept_messages_when_the_recent_window_moves_past_them() {
    let mut fixture = Fixture::default();
    let u1 = fixture.message_entry(user_message(
        &"user msg 1 (summarized by compaction1)".repeat(4),
    ));
    let a1 = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        &"assistant msg 1".repeat(4),
    )));
    let u2 = fixture.message_entry(user_message(
        &"user msg 2 - kept by compaction1 ".repeat(12),
    ));
    let a2 = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        &"assistant msg 2 ".repeat(12),
    )));
    let u3 = fixture.message_entry(user_message(
        &"user msg 3 - kept by compaction1 ".repeat(12),
    ));
    let a3 = fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
        &"assistant msg 3 ".repeat(12),
        mock_usage(5000, 1000),
    )));
    let compaction1 = fixture.compaction_entry("First summary", u2.id().unwrap());
    let u4 = fixture.message_entry(user_message(
        &"user msg 4 (new after compaction1) ".repeat(12),
    ));
    let a4 = fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
        &"assistant msg 4 ".repeat(12),
        mock_usage(8000, 2000),
    )));

    let settings = CompactionSettings {
        keep_recent_tokens: 100,
        ..DEFAULT_COMPACTION_SETTINGS
    };
    let preparation = prepare_compaction(&[u1, a1, u2, a2, u3, a3, compaction1, u4, a4], settings)
        .expect("preparation");

    let summarized_text = extract_text(&preparation.messages_to_summarize);
    assert!(summarized_text.contains("user msg 2 - kept by compaction1"));
    assert!(summarized_text.contains("user msg 3 - kept by compaction1"));
    assert!(!summarized_text.contains("First summary"));
    assert_eq!(
        preparation.previous_summary.as_deref(),
        Some("First summary")
    );
}

// ---------------------------------------------------------------------------
// Ported spec: compaction-serialization.test.ts
// ---------------------------------------------------------------------------

fn tool_result_message(text: &str) -> Message {
    Message::ToolResult(crate::ai::types::ToolResultMessage {
        tool_call_id: "tc1".to_string(),
        tool_name: "read".to_string(),
        content: vec![TextOrImageBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })],
        details: None,
        usage: None,
        is_error: false,
        timestamp: TS,
    })
}

#[test]
fn truncates_long_tool_results() {
    let long_content = "x".repeat(5000);
    let messages = vec![tool_result_message(&long_content)];

    let result = serialize_conversation(&messages);

    assert!(result.contains("[Tool result]:"));
    assert!(result.contains("[... 3000 more characters truncated]"));
    assert!(!result.contains(&"x".repeat(3000)));
    assert!(result.contains(&"x".repeat(2000)));
}

#[test]
fn does_not_truncate_short_tool_results() {
    let short_content = "x".repeat(1500);
    let messages = vec![tool_result_message(&short_content)];

    let result = serialize_conversation(&messages);

    assert_eq!(result, format!("[Tool result]: {short_content}"));
    assert!(!result.contains("truncated"));
}

#[test]
fn does_not_truncate_assistant_or_user_messages() {
    let long_text = "y".repeat(5000);
    let messages = vec![
        Message::User(UserMessage {
            content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                text: long_text.clone(),
                text_signature: None,
            })]),
            timestamp: TS,
        }),
        Message::Assistant(AssistantMessage {
            content: vec![text_block(&long_text)],
            ..text_response("ignored")
        }),
    ];

    let result = serialize_conversation(&messages);

    assert!(!result.contains("truncated"));
    assert!(result.contains(&long_text));
}

// ---------------------------------------------------------------------------
// Ported spec: branch-summarization.test.ts
// ---------------------------------------------------------------------------

fn branch_entries() -> Vec<SessionEntry> {
    vec![SessionEntry::Message {
        id: "branch-user".to_string(),
        parent_id: None,
        message: user_message_at("Abandoned request", 1),
    }]
}

#[tokio::test]
async fn does_not_override_tool_choice_for_branch_summaries() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![text_response("summary")], &calls);
    let mut callbacks = RetryCallbacks::default();

    let result = generate_branch_summary(
        &branch_entries(),
        branch_options(&test_model(), stream_fn, &mut callbacks),
    )
    .await;

    assert!(result.error.is_none());
    let (context, options) = &calls_of(&calls)[0];
    assert_eq!(options["maxTokens"], 4096);
    assert!(options.get("toolChoice").is_none());
    // The prompt is the serialized conversation wrapped in tags, above the
    // branch summary instructions.
    let prompt = context["messages"][1]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(prompt.contains("[User]: Abandoned request"));
    assert!(prompt.contains("Create a structured summary of this conversation branch"));
}

#[tokio::test]
async fn clamps_the_branch_summary_output_cap_to_the_model_limit() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![text_response("summary")], &calls);
    let mut callbacks = RetryCallbacks::default();

    generate_branch_summary(
        &branch_entries(),
        branch_options(&test_model_with(false, 1024), stream_fn, &mut callbacks),
    )
    .await;

    assert_eq!(calls_of(&calls)[0].1["maxTokens"], 1024);
}

#[tokio::test]
async fn rejects_tool_calls_from_branch_summaries() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![tool_call_response()], &calls);
    let mut callbacks = RetryCallbacks::default();

    let result = generate_branch_summary(
        &branch_entries(),
        branch_options(&test_model(), stream_fn, &mut callbacks),
    )
    .await;

    assert_eq!(
        result.error.as_deref(),
        Some("Branch summarization attempted to call a tool")
    );
}

#[tokio::test]
async fn rejects_length_limited_branch_summaries() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(
        vec![AssistantMessage {
            stop_reason: StopReason::Length,
            ..text_response("partial")
        }],
        &calls,
    );
    let mut callbacks = RetryCallbacks::default();

    let result = generate_branch_summary(
        &branch_entries(),
        branch_options(&test_model(), stream_fn, &mut callbacks),
    )
    .await;

    assert_eq!(
        result.error.as_deref(),
        Some(
            "Branch summarization failed: generation hit the token cap and the summary is incomplete"
        )
    );
}

#[tokio::test]
async fn reports_aborted_branch_summaries() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(
        vec![AssistantMessage {
            stop_reason: StopReason::Aborted,
            content: Vec::new(),
            ..text_response("ignored")
        }],
        &calls,
    );
    let mut callbacks = RetryCallbacks::default();

    let result = generate_branch_summary(
        &branch_entries(),
        branch_options(&test_model(), stream_fn, &mut callbacks),
    )
    .await;

    assert_eq!(result.aborted, Some(true));
    assert!(result.summary.is_none());
}

#[tokio::test]
async fn returns_no_content_to_summarize_for_empty_branches() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![text_response("never called")], &calls);
    let mut callbacks = RetryCallbacks::default();

    let result = generate_branch_summary(
        &[SessionEntry::Other],
        branch_options(&test_model(), stream_fn, &mut callbacks),
    )
    .await;

    assert_eq!(result.summary.as_deref(), Some("No content to summarize"));
    assert!(calls_of(&calls).is_empty());
}

#[tokio::test]
async fn replaces_instructions_when_asked() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![text_response("replaced")], &calls);
    let mut callbacks = RetryCallbacks::default();

    generate_branch_summary(
        &branch_entries(),
        GenerateBranchSummaryOptions {
            custom_instructions: Some("Only list files"),
            replace_instructions: true,
            ..branch_options(&test_model(), stream_fn, &mut callbacks)
        },
    )
    .await;

    let (context, _options) = &calls_of(&calls)[0];
    let prompt = context["messages"][1]["content"][0]["text"]
        .as_str()
        .unwrap();
    // Replace mode uses ONLY the custom instructions after the conversation.
    assert!(prompt.ends_with("\n\nOnly list files"));
    assert!(!prompt.contains("Create a structured summary of this conversation branch"));
}

// ---------------------------------------------------------------------------
// Ported spec: compaction-summary-reasoning.test.ts (completeSimple mock →
// scripted StreamFn)
// ---------------------------------------------------------------------------

fn response_usage() -> Usage {
    Usage {
        input: 10,
        output: 10,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 20,
        cost: UsageCost::default(),
    }
}

fn mock_summary_response() -> AssistantMessage {
    AssistantMessage {
        content: vec![text_block("## Goal\nTest summary")],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        usage: response_usage(),
        stop_reason: StopReason::Stop,
        timestamp: TS,
        ..text_response("unused")
    }
}

fn summary_messages() -> Vec<AgentMessage> {
    vec![user_message("Summarize this.")]
}

#[tokio::test]
async fn uses_the_provided_thinking_level_for_reasoning_capable_models() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![mock_summary_response()], &calls);
    let mut callbacks = RetryCallbacks::default();

    let result = generate_summary_with_usage(SummaryRequest {
        thinking_level: Some(ThinkingLevel::Medium),
        ..summary_request(
            &summary_messages(),
            &test_model_with(true, 8192),
            2000,
            Some("test-key"),
            stream_fn,
            &mut callbacks,
        )
    })
    .await
    .expect("summary");

    assert_eq!(result.text, "## Goal\nTest summary");
    assert_eq!(result.usage, response_usage());
    let logged = calls_of(&calls);
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].1["reasoning"], "medium");
    assert_eq!(logged[0].1["apiKey"], "test-key");
}

#[tokio::test]
async fn preserves_the_string_result_from_generate_summary() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![mock_summary_response()], &calls);
    let mut callbacks = RetryCallbacks::default();

    let text = generate_summary(summary_request(
        &summary_messages(),
        &test_model(),
        2000,
        Some("test-key"),
        stream_fn,
        &mut callbacks,
    ))
    .await
    .expect("summary");

    assert_eq!(text, "## Goal\nTest summary");
}

#[tokio::test]
async fn fresh_routing_ids_differ_between_calls() {
    let raw_ids: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let ids = Arc::clone(&raw_ids);
    let scripted: StreamFn = Arc::new(move |_model, _context, options| {
        ids.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(options.stream.session_id.clone());
        Box::pin(async move { text_response("ok") })
    });
    let mut callbacks = RetryCallbacks::default();

    for _ in 0..2 {
        generate_summary(SummaryRequest {
            stream_fn: Some(Arc::clone(&scripted)),
            ..summary_request(
                &summary_messages(),
                &test_model(),
                2000,
                Some("test-key"),
                None,
                &mut callbacks,
            )
        })
        .await
        .expect("summary");
    }

    let ids = raw_ids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(ids.len(), 2);
    assert!(ids[0] != ids[1]);
    assert!(ids
        .iter()
        .all(|id| id.as_deref().map(is_uuid_v7).unwrap_or(false)));
}

#[tokio::test]
async fn honors_caller_supplied_routing_session_and_tool_choice_without_prompt_caching() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![text_response("ok")], &calls);
    let mut callbacks = RetryCallbacks::default();

    complete_summarization(
        &test_model(),
        crate::ai::transcript::normalize_context(&crate::ai::transcript::Context {
            system_prompt: Some("Summarize".to_string()),
            messages: Vec::new(),
            tools: None,
        }),
        SimpleStreamOptions {
            stream: crate::ai::types::StreamOptions {
                session_id: Some("current-routing-session".to_string()),
                cache_retention: Some(CacheRetention::Short),
                max_tokens: Some(999),
                ..crate::ai::types::StreamOptions::default()
            },
            tool_choice: Some(ToolChoice::Auto),
            ..SimpleStreamOptions::default()
        },
        stream_fn,
        None,
        &mut callbacks,
    )
    .await;

    let options = &calls_of(&calls)[0].1;
    assert_eq!(options["sessionId"], "current-routing-session");
    assert_eq!(options["cacheRetention"], "none");
    assert_eq!(options["toolChoice"], "auto");
    assert_eq!(options["maxTokens"], 999);
}

#[tokio::test]
async fn preserves_the_previous_summary_without_an_empty_history_request_for_a_split_turn() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![text_response("## Goal\nPrefix only")], &calls);
    let mut callbacks = RetryCallbacks::default();

    let result = compact(
        split_preparation(Vec::new(), Some("previous checkpoint")),
        compact_options(&test_model(), stream_fn, &mut callbacks),
    )
    .await
    .expect("compact");

    // Only the turn prefix is requested; the previous summary rides along.
    assert!(result.summary.contains("previous checkpoint"));
    assert_eq!(calls_of(&calls).len(), 1);
}

#[tokio::test]
async fn rejects_tool_calls_from_conversation_summaries() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![tool_call_response()], &calls);
    let mut callbacks = RetryCallbacks::default();

    let error = generate_summary_with_usage(summary_request(
        &summary_messages(),
        &test_model(),
        2000,
        Some("test-key"),
        stream_fn,
        &mut callbacks,
    ))
    .await
    .expect_err("tool call rejected");

    assert_eq!(error, "Summarization attempted to call a tool");
}

#[tokio::test]
async fn rejects_tool_calls_from_split_turn_summaries() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![tool_call_response()], &calls);
    let mut callbacks = RetryCallbacks::default();

    let error = compact(
        split_preparation(Vec::new(), None),
        compact_options(&test_model(), stream_fn, &mut callbacks),
    )
    .await
    .expect_err("tool call rejected");

    assert_eq!(error, "Turn prefix summarization attempted to call a tool");
}

#[tokio::test]
async fn rejects_a_length_limited_history_summary() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(
        vec![AssistantMessage {
            stop_reason: StopReason::Length,
            ..text_response("partial")
        }],
        &calls,
    );
    let mut callbacks = RetryCallbacks::default();

    let error = generate_summary_with_usage(summary_request(
        &summary_messages(),
        &test_model(),
        2000,
        Some("test-key"),
        stream_fn,
        &mut callbacks,
    ))
    .await
    .expect_err("length stop rejected");

    assert!(error.contains("generation hit the token cap"));
}

#[tokio::test]
async fn rejects_a_length_limited_split_turn_summary() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(
        vec![AssistantMessage {
            stop_reason: StopReason::Length,
            ..text_response("partial")
        }],
        &calls,
    );
    let mut callbacks = RetryCallbacks::default();

    let error = compact(
        split_preparation(Vec::new(), None),
        compact_options(&test_model(), stream_fn, &mut callbacks),
    )
    .await
    .expect_err("length stop rejected");

    assert!(error.contains("generation hit the token cap"));
}

#[tokio::test]
async fn does_not_set_reasoning_when_thinking_is_off() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![text_response("ok")], &calls);
    let mut callbacks = RetryCallbacks::default();

    generate_summary(SummaryRequest {
        thinking_level: Some(ThinkingLevel::Off),
        ..summary_request(
            &summary_messages(),
            &test_model_with(true, 8192),
            2000,
            Some("test-key"),
            stream_fn,
            &mut callbacks,
        )
    })
    .await
    .expect("summary");

    let options = &calls_of(&calls)[0].1;
    assert_eq!(options["apiKey"], "test-key");
    assert!(options.get("reasoning").is_none());
}

#[tokio::test]
async fn does_not_set_reasoning_for_non_reasoning_models() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![text_response("ok")], &calls);
    let mut callbacks = RetryCallbacks::default();

    generate_summary(SummaryRequest {
        thinking_level: Some(ThinkingLevel::Medium),
        ..summary_request(
            &summary_messages(),
            &test_model(),
            2000,
            Some("test-key"),
            stream_fn,
            &mut callbacks,
        )
    })
    .await
    .expect("summary");

    let options = &calls_of(&calls)[0].1;
    assert_eq!(options["apiKey"], "test-key");
    assert!(options.get("reasoning").is_none());
}

#[tokio::test]
async fn clamps_compaction_summary_max_tokens_to_the_model_output_cap() {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(
        vec![mock_summary_response(), mock_summary_response()],
        &calls,
    );
    let mut callbacks = RetryCallbacks::default();

    let messages = summary_messages();
    let preparation = CompactionPreparation {
        first_kept_entry_id: "entry-keep".to_string(),
        messages_to_summarize: messages.clone(),
        turn_prefix_messages: messages,
        is_split_turn: true,
        tokens_before: 600_000,
        previous_summary: None,
        file_ops: FileOperations::new(),
        settings: CompactionSettings {
            enabled: true,
            reserve_tokens: 500_000,
            keep_recent_tokens: 20_000,
        },
    };

    let result = compact(
        preparation,
        compact_options(&test_model_with(false, 128_000), stream_fn, &mut callbacks),
    )
    .await
    .expect("compact");

    // Both summarization calls (history + turn prefix) clamp to the model cap
    // and their usage sums componentwise.
    let usage = result.usage.expect("usage");
    assert_eq!(usage.input, 20);
    assert_eq!(usage.output, 20);
    assert_eq!(usage.total_tokens, 40);
    assert_eq!(usage.cost, UsageCost::default());
    let max_tokens: Vec<Value> = calls_of(&calls)
        .iter()
        .map(|(_, options)| options["maxTokens"].clone())
        .collect();
    assert_eq!(max_tokens, vec![json!(128_000), json!(128_000)]);
}

// ---------------------------------------------------------------------------
// Unit spec: getSummarizationFailure + transport seams
// ---------------------------------------------------------------------------

#[test]
fn summarization_failure_messages_match_upstream_labels() {
    let mut failed = text_response("x");
    failed.stop_reason = StopReason::Error;
    failed.error_message = Some("socket closed".to_string());
    assert_eq!(
        get_summarization_failure(&failed, "Summarization").as_deref(),
        Some("Summarization failed: socket closed")
    );
    assert_eq!(
        get_summarization_failure(&failed, "Branch summarization").as_deref(),
        Some("Branch summarization failed: socket closed")
    );

    let mut failed_no_message = failed.clone();
    failed_no_message.error_message = None;
    assert_eq!(
        get_summarization_failure(&failed_no_message, "Summarization").as_deref(),
        Some("Summarization failed: Unknown error")
    );

    let mut truncated = text_response("partial");
    truncated.stop_reason = StopReason::Length;
    assert_eq!(
        get_summarization_failure(&truncated, "Summarization").as_deref(),
        Some("Summarization failed: generation hit the token cap and the summary is incomplete")
    );
    assert_eq!(
        get_summarization_failure(&truncated, "Turn prefix summarization").as_deref(),
        Some(
            "Turn prefix summarization failed: generation hit the token cap and the summary is incomplete"
        )
    );

    let ok = text_response("fine");
    assert_eq!(get_summarization_failure(&ok, "Summarization"), None);
}

#[tokio::test]
async fn unbound_transport_surfaces_an_error_assistant_message() {
    let mut callbacks = RetryCallbacks::default();
    let response = complete_summarization(
        &test_model(),
        build_summarization_context("prompt".to_string()),
        SimpleStreamOptions::default(),
        None,
        None,
        &mut callbacks,
    )
    .await;

    assert_eq!(response.stop_reason, StopReason::Error);
    assert!(response
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("compaction transport unbound"));
}

#[tokio::test]
async fn models_stream_fn_binds_the_models_collection_transport() {
    let faux = faux_provider(FauxProviderOptions {
        provider: Some("faux-compaction".to_string()),
        models: vec![FauxModelDefinition {
            id: "faux-compaction-1".to_string(),
            context_window: Some(200_000),
            max_tokens: Some(8192),
            ..FauxModelDefinition::default()
        }],
        ..FauxProviderOptions::default()
    });
    let model = faux.get_model(None).expect("faux model");
    faux.set_responses(vec![FauxResponseStep::Message(Box::new(text_response(
        "## Goal\nRouted summary",
    )))]);
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(faux.provider.clone());

    let mut callbacks = RetryCallbacks::default();
    let response = complete_summarization(
        &model,
        build_summarization_context("prompt".to_string()),
        SimpleStreamOptions::default(),
        Some(models_stream_fn(Arc::new(models))),
        None,
        &mut callbacks,
    )
    .await;

    assert_eq!(response.stop_reason, StopReason::Stop);
    match response.content.first() {
        Some(AssistantBlock::Text(text)) => assert_eq!(text.text, "## Goal\nRouted summary"),
        other => panic!("expected text block, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Seam spec: collectEntriesForBranchSummary (ReadonlySessionManager trait)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeSession {
    entries: HashMap<String, SessionEntry>,
    last_id: Option<String>,
}

impl FakeSession {
    fn chain(&mut self, ids: &[&str]) {
        for id in ids {
            let entry = SessionEntry::Message {
                id: id.to_string(),
                parent_id: self.last_id.clone(),
                message: user_message(&format!("message {id}")),
            };
            self.last_id = Some(id.to_string());
            self.entries.insert(id.to_string(), entry);
        }
    }
}

impl ReadonlySessionManager for FakeSession {
    fn get_branch(&self, id: &str) -> Vec<SessionEntry> {
        let mut branch = Vec::new();
        let mut current = Some(id.to_string());
        while let Some(entry_id) = current {
            let Some(entry) = self.entries.get(&entry_id) else {
                break;
            };
            current = entry.parent_id().map(str::to_string);
            branch.push(entry.clone());
        }
        branch.reverse();
        branch
    }

    fn get_entry(&self, id: &str) -> Option<SessionEntry> {
        self.entries.get(id).cloned()
    }
}

#[test]
fn collects_entries_from_old_leaf_back_to_the_common_ancestor() {
    let mut session = FakeSession::default();
    session.chain(&["e1", "e2", "e3", "e4-old"]);
    let target_entry = SessionEntry::Message {
        id: "e5-target".to_string(),
        parent_id: Some("e1".to_string()),
        message: user_message("target message"),
    };
    session
        .entries
        .insert("e5-target".to_string(), target_entry);

    let result = collect_entries_for_branch_summary(&session, Some("e4-old"), "e5-target");

    let ids: Vec<&str> = result
        .entries
        .iter()
        .filter_map(|entry| entry.id())
        .collect();
    assert_eq!(ids, vec!["e2", "e3", "e4-old"]);
    assert_eq!(result.common_ancestor_id.as_deref(), Some("e1"));
}

#[test]
fn collects_nothing_without_an_old_position() {
    let mut session = FakeSession::default();
    session.chain(&["e1"]);
    let result = collect_entries_for_branch_summary(&session, None, "e1");
    assert!(result.entries.is_empty());
    assert_eq!(result.common_ancestor_id, None);
}

// ---------------------------------------------------------------------------
// Oracle comparisons: byte-level equivalence against the upstream capture
// ---------------------------------------------------------------------------

#[test]
fn oracle_constants_match() {
    let oracle = oracle();
    assert_eq!(
        SUMMARIZATION_SYSTEM_PROMPT,
        oracle["constants"]["summarizationSystemPrompt"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        canon_string(&serde_json::to_value(DEFAULT_COMPACTION_SETTINGS).unwrap()),
        canon_string(&oracle["constants"]["defaultSettings"])
    );
}

#[test]
fn oracle_file_ops_match() {
    let oracle = oracle()["fileOps"].clone();
    let mut file_ops = FileOperations::new();
    extract_file_ops_from_message(
        &AgentMessage::Assistant(assistant_message("x")),
        &mut file_ops,
    );
    extract_file_ops_from_message(&user_message("y"), &mut file_ops);
    extract_file_ops_from_message(
        &AgentMessage::Assistant(AssistantMessage {
            content: vec![
                text_block("hi"),
                tool_call_block("read", json!({"path": "src/a.ts"})),
                tool_call_block("write", json!({"path": "src/b.ts"})),
                tool_call_block("edit", json!({"path": "src/c.ts"})),
                tool_call_block("grep", json!({"path": "src/d.ts"})),
                tool_call_block("read", json!({})),
                AssistantBlock::ToolCall(ToolCall {
                    id: "call-read-no-arguments".to_string(),
                    name: "read".to_string(),
                    arguments: Value::Null,
                    thought_signature: None,
                    namespace: None,
                }),
                tool_call_block("read", json!({"path": 42})),
            ],
            ..text_response("x")
        }),
        &mut file_ops,
    );
    extract_file_ops_from_message(
        &AgentMessage::Assistant(AssistantMessage {
            content: vec![tool_call_block("read", json!({"path": "src/e.ts"}))],
            ..text_response("x")
        }),
        &mut file_ops,
    );

    assert_eq!(
        file_ops_json(&file_ops),
        oracle["opsJson"].as_str().unwrap()
    );

    let lists = compute_file_lists(&file_ops);
    assert_eq!(
        canon_string(&serde_json::to_value(&lists).unwrap()),
        canon_string(&oracle["lists"])
    );

    assert_eq!(
        format_file_operations(
            &["README.md".to_string(), "src/a.ts".to_string()],
            &["src/b.ts".to_string(), "src/c.ts".to_string()]
        ),
        oracle["formattedBoth"].as_str().unwrap()
    );
    assert_eq!(
        format_file_operations(&["only-read.txt".to_string()], &[]),
        oracle["formattedRead"].as_str().unwrap()
    );
    assert_eq!(
        format_file_operations(&[], &["changed.txt".to_string()]),
        oracle["formattedModified"].as_str().unwrap()
    );
    assert_eq!(
        format_file_operations(&[], &[]),
        oracle["formattedNone"].as_str().unwrap()
    );
}

#[test]
fn oracle_serialize_conversation_matches() {
    let oracle = oracle()["serialize"].clone();
    let long = "x".repeat(5000);
    let long_y = "y".repeat(5000);

    let assistant_plain = |text: &str| AssistantMessage {
        content: vec![text_block(text)],
        api: "anthropic".to_string(),
        provider: "anthropic".to_string(),
        model: "test".to_string(),
        usage: mock_usage(0, 0),
        ..text_response("unused")
    };

    assert_eq!(
        serialize_conversation(&[tool_result_message(&long)]),
        oracle["longToolResult"].as_str().unwrap()
    );
    assert_eq!(
        serialize_conversation(&[tool_result_message(&"x".repeat(1500))]),
        oracle["shortToolResult"].as_str().unwrap()
    );
    assert_eq!(
        serialize_conversation(&[tool_result_message(&"x".repeat(2000))]),
        oracle["exactBoundary"].as_str().unwrap()
    );
    assert_eq!(
        serialize_conversation(&[tool_result_message(&"x".repeat(2001))]),
        oracle["overBoundary"].as_str().unwrap()
    );
    assert_eq!(
        serialize_conversation(&[
            Message::User(UserMessage {
                content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                    text: long_y.clone(),
                    text_signature: None,
                })]),
                timestamp: TS,
            }),
            Message::Assistant(assistant_plain(&long_y)),
        ]),
        oracle["userAndAssistantLong"].as_str().unwrap()
    );
    assert_eq!(
        serialize_conversation(&[Message::Assistant(AssistantMessage {
            content: vec![
                AssistantBlock::Thinking(ThinkingContent {
                    thinking: "Thought A".to_string(),
                    thinking_signature: None,
                    redacted: None,
                }),
                text_block("Answer"),
                tool_call_block("read", json!({"path": "a.txt"})),
                tool_call_block("edit", json!({"a": 1, "b": "x"})),
            ],
            ..assistant_plain("unused")
        })]),
        oracle["assistantThinkingToolCalls"].as_str().unwrap()
    );
    assert_eq!(
        serialize_conversation(&[
            Message::User(UserMessage {
                content: StringOrBlocks::Text(String::new()),
                timestamp: TS,
            }),
            Message::User(UserMessage {
                content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                    text: "visible".to_string(),
                    text_signature: None,
                })]),
                timestamp: TS,
            }),
        ]),
        oracle["emptyUserContentSkipped"].as_str().unwrap()
    );
    assert_eq!(
        serialize_conversation(&[
            Message::User(UserMessage {
                content: StringOrBlocks::Text("one".to_string()),
                timestamp: TS,
            }),
            tool_result_message("out"),
            Message::User(UserMessage {
                content: StringOrBlocks::Text("two".to_string()),
                timestamp: TS,
            }),
        ]),
        oracle["joinedWithBlankLines"].as_str().unwrap()
    );
    assert_eq!(
        serialize_conversation(&[]),
        oracle["empty"].as_str().unwrap()
    );
}

#[test]
fn oracle_token_calculations_match() {
    let oracle = oracle()["tokenCalc"].clone();
    let third = Usage {
        total_tokens: 555,
        ..mock_usage_full(100, 10, 0, 0)
    };
    let results = vec![
        calculate_context_tokens(mock_usage_full(1000, 500, 200, 100)),
        calculate_context_tokens(mock_usage(0, 0)),
        calculate_context_tokens(third),
    ];
    let expected: Vec<u64> = oracle["calculate"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_u64().unwrap())
        .collect();
    assert_eq!(results, expected);
}

#[test]
fn oracle_last_assistant_usage_matches() {
    let oracle = oracle()["lastAssistantUsage"].clone();
    let mut fixture = Fixture::default();
    let last_non_aborted = vec![
        fixture.message_entry(user_message("Hello")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Hi",
            mock_usage(100, 50),
        ))),
        fixture.message_entry(user_message("How are you?")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Good",
            mock_usage(200, 100),
        ))),
    ];
    fixture.reset();
    let mut aborted = assistant_message_with_usage("Aborted", mock_usage(300, 150));
    aborted.stop_reason = StopReason::Aborted;
    let skips_aborted = vec![
        fixture.message_entry(user_message("Hello")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Hi",
            mock_usage(100, 50),
        ))),
        fixture.message_entry(user_message("How are you?")),
        fixture.message_entry(AgentMessage::Assistant(aborted)),
    ];
    fixture.reset();
    let skips_all_zero = vec![
        fixture.message_entry(user_message("Hello")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Hi",
            mock_usage(100, 50),
        ))),
        fixture.message_entry(user_message("continue")),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Partial",
            mock_usage(0, 0),
        ))),
    ];
    fixture.reset();
    let mut failed = assistant_message_with_usage("Err", mock_usage(500, 50));
    failed.stop_reason = StopReason::Error;
    failed.error_message = Some("boom".to_string());
    let error_stop_skipped = vec![
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "Bad",
            mock_usage(400, 40),
        ))),
        fixture.message_entry(AgentMessage::Assistant(failed)),
    ];
    fixture.reset();
    let no_assistant = vec![fixture.message_entry(user_message("Hello"))];
    fixture.reset();
    let includes_non_message_entries = vec![
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            "A",
            mock_usage(111, 11),
        ))),
        {
            let _ = fixture.base_entry();
            SessionEntry::Other
        },
        fixture.message_entry(user_message("q")),
    ];

    let cases: Vec<(&str, Vec<SessionEntry>)> = vec![
        ("last_non_aborted", last_non_aborted),
        ("skips_aborted", skips_aborted),
        ("skips_all_zero", skips_all_zero),
        ("error_stop_skipped", error_stop_skipped),
        ("no_assistant", no_assistant),
        ("includes_non_message_entries", includes_non_message_entries),
    ];

    for (name, entries) in cases {
        let expected = &oracle[name];
        match get_last_assistant_usage(&entries) {
            Some(usage) => assert_eq!(usage_json(&usage), canon_string(expected), "{name}"),
            None => assert!(expected.is_null(), "{name}"),
        }
    }
}

#[test]
fn oracle_estimate_context_tokens_matches() {
    let oracle = oracle()["estimateContextTokens"].clone();

    let estimate_json = |estimate: &ContextUsageEstimate| {
        json!({
            "tokens": estimate.tokens,
            "usageTokens": estimate.usage_tokens,
            "trailingTokens": estimate.trailing_tokens,
            "lastUsageIndex": estimate
                .last_usage_index
                .map_or(Value::Null, |index| json!(index)),
        })
    };

    let anchored = estimate_context_tokens(&[
        user_message("Hello"),
        AgentMessage::Assistant(assistant_message_with_usage("Hi", mock_usage(100, 50))),
        user_message("continue"),
        AgentMessage::Assistant(assistant_message_with_usage(
            "Partial thinking",
            mock_usage(0, 0),
        )),
    ]);
    assert_eq!(
        canon_string(&estimate_json(&anchored)),
        canon_string(&oracle["anchored"])
    );

    let no_usage = estimate_context_tokens(&[
        user_message("only"),
        AgentMessage::Assistant(assistant_message_with_usage("zero", mock_usage(0, 0))),
    ]);
    assert_eq!(
        canon_string(&estimate_json(&no_usage)),
        canon_string(&oracle["noUsage"])
    );

    let empty = estimate_context_tokens(&[]);
    assert_eq!(
        canon_string(&estimate_json(&empty)),
        canon_string(&oracle["empty"])
    );
}

#[test]
fn oracle_estimate_tokens_matches() {
    let oracle = oracle()["estimateTokens"].clone();
    let battery = vec![
        ("userString", estimate_tokens(&user_message("12345678"))),
        (
            "userBlocks",
            estimate_tokens(&AgentMessage::User(
                serde_json::from_value(json!({
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "12345678"},
                        {"type": "image", "data": "aaaa", "mimeType": "image/png"}
                    ],
                    "timestamp": TS
                }))
                .unwrap(),
            )),
        ),
        (
            "assistantBlocks",
            estimate_tokens(&AgentMessage::Assistant(AssistantMessage {
                content: vec![
                    text_block("12345678"),
                    AssistantBlock::Thinking(ThinkingContent {
                        thinking: "1234".to_string(),
                        thinking_signature: None,
                        redacted: None,
                    }),
                    tool_call_block("read", json!({"path": "a.txt"})),
                ],
                api: "a".to_string(),
                provider: "p".to_string(),
                model: "m".to_string(),
                usage: mock_usage(0, 0),
                stop_reason: StopReason::Stop,
                timestamp: TS,
                ..text_response("unused")
            })),
        ),
        (
            "toolResult",
            estimate_tokens(&AgentMessage::ToolResult(
                serde_json::from_value(json!({
                    "role": "toolResult",
                    "toolCallId": "t",
                    "toolName": "read",
                    "content": [
                        {"type": "text", "text": "12345678"},
                        {"type": "image", "data": "bbbb", "mimeType": "image/png"}
                    ],
                    "isError": false,
                    "timestamp": TS
                }))
                .unwrap(),
            )),
        ),
        (
            "bashExecution",
            estimate_tokens(
                &serde_json::from_value(json!({
                    "role": "bashExecution",
                    "command": "ls -la",
                    "output": "total 0",
                    "exitCode": 0,
                    "cancelled": false,
                    "truncated": false,
                    "timestamp": TS
                }))
                .unwrap(),
            ),
        ),
        (
            "branchSummary",
            estimate_tokens(
                &serde_json::from_value(json!({
                    "role": "branchSummary",
                    "summary": "12345678",
                    "fromId": null,
                    "timestamp": TS
                }))
                .unwrap(),
            ),
        ),
        (
            "compactionSummary",
            estimate_tokens(
                &serde_json::from_value(json!({
                    "role": "compactionSummary",
                    "summary": "1234",
                    "tokensBefore": 5,
                    "timestamp": TS
                }))
                .unwrap(),
            ),
        ),
        (
            "custom",
            estimate_tokens(
                &serde_json::from_value(json!({
                    "role": "custom",
                    "customType": "x",
                    "content": "12345678",
                    "display": true,
                    "timestamp": TS
                }))
                .unwrap(),
            ),
        ),
        (
            "system",
            estimate_tokens(
                &serde_json::from_value(json!({
                    "role": "system",
                    "content": "12345678",
                    "timestamp": TS
                }))
                .unwrap(),
            ),
        ),
    ];
    for (name, value) in battery {
        assert_eq!(value, oracle[name].as_u64().unwrap(), "{name}");
    }
}

#[test]
fn oracle_should_compact_matches() {
    let oracle = oracle()["shouldCompact"].clone();
    let settings = |enabled| CompactionSettings {
        enabled,
        reserve_tokens: 10_000,
        keep_recent_tokens: 20_000,
    };
    let results = vec![
        should_compact(95_000, 100_000, settings(true)),
        should_compact(89_000, 100_000, settings(true)),
        should_compact(95_000, 100_000, settings(false)),
        should_compact(89_999, 100_000, settings(true)),
        should_compact(90_000, 100_000, settings(true)),
        should_compact(90_001, 100_000, settings(true)),
    ];
    let expected: Vec<bool> = oracle
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_bool().unwrap())
        .collect();
    assert_eq!(results, expected);
}

#[test]
fn oracle_find_cut_point_matches() {
    let oracle = oracle()["findCutPoint"].clone();
    let batteries = cut_point_fixture();
    let token_diff = &batteries[0];
    let single_assistant = &batteries[1];
    let all_fit = &batteries[2];
    let split_turn = &batteries[3];
    let custom_budget = &batteries[4];
    let metadata_scan = &batteries[5];
    let tool_results_never_cut = &batteries[6];

    let cases: Vec<(&str, CutPointResult)> = vec![
        (
            "token_diff",
            find_cut_point(token_diff, 0, token_diff.len(), 2500),
        ),
        (
            "no_valid_cut_points",
            find_cut_point(single_assistant, 0, single_assistant.len(), 1000),
        ),
        ("all_fit", find_cut_point(all_fit, 0, all_fit.len(), 50_000)),
        (
            "split_turn",
            find_cut_point(split_turn, 0, split_turn.len(), 250),
        ),
        (
            "tiny_budget_custom",
            find_cut_point(custom_budget, 0, custom_budget.len(), 1),
        ),
        (
            "custom_fits",
            find_cut_point(custom_budget, 0, custom_budget.len(), 2),
        ),
        (
            "metadata_back_scan",
            find_cut_point(metadata_scan, 0, metadata_scan.len(), 640),
        ),
        (
            "tool_results_never_cut",
            find_cut_point(tool_results_never_cut, 0, tool_results_never_cut.len(), 100),
        ),
        (
            "zero_keep_budget",
            find_cut_point(all_fit, 0, all_fit.len(), 0),
        ),
        (
            "range_limited",
            find_cut_point(token_diff, 10, token_diff.len(), 2500),
        ),
    ];

    for (name, result) in cases {
        assert_eq!(cut_json(result), oracle[name], "{name}");
    }
}

#[test]
fn oracle_find_turn_start_index_matches() {
    let oracle = oracle()["findTurnStartIndex"].clone();
    let batteries = cut_point_fixture();
    let split_turn = &batteries[3];
    let all_fit = &batteries[2];
    let custom_budget = &batteries[4];

    let cases: Vec<(&str, Option<usize>)> = vec![
        ("mid_turn", find_turn_start_index(split_turn, 5, 0)),
        ("at_user", find_turn_start_index(split_turn, 2, 0)),
        ("before_start", find_turn_start_index(split_turn, 1, 2)),
        ("first_entry", find_turn_start_index(all_fit, 0, 0)),
        (
            "custom_entry_is_turn_start",
            find_turn_start_index(custom_budget, 3, 0),
        ),
    ];

    for (name, result) in cases {
        let expected = oracle[name].as_i64().unwrap();
        assert_eq!(result.map_or(-1, |index| index as i64), expected, "{name}");
    }
}

#[test]
fn oracle_prepare_compaction_matches() {
    let oracle = oracle()["prepareCompaction"].clone();

    // system-message entries are prompt state, not conversation history
    let mut fixture = Fixture::default();
    let turn_message = user_message("one long turn");
    let system_entry = fixture.message_entry(AgentMessage::System(
        serde_json::from_value(json!({
            "role": "system",
            "content": "",
            "sections": {"preamble": "current prompt"},
            "timestamp": TS
        }))
        .unwrap(),
    ));
    let user_entry = fixture.message_entry(turn_message.clone());
    let assistant_entry = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        "assistant suffix",
    )));
    let system_id = system_entry.id().unwrap().to_string();
    let user_id = user_entry.id().unwrap().to_string();
    let assistant_id = assistant_entry.id().unwrap().to_string();
    let preparation = prepare_compaction(
        &[system_entry, user_entry, assistant_entry],
        CompactionSettings {
            keep_recent_tokens: 1,
            ..DEFAULT_COMPACTION_SETTINGS
        },
    )
    .expect("preparation");
    let captured = &oracle["system_messages_skipped"];
    assert_eq!(
        preparation.first_kept_entry_id,
        captured["firstKeptEntryId"].as_str().unwrap()
    );
    assert_eq!(
        preparation.is_split_turn,
        captured["isSplitTurn"].as_bool().unwrap()
    );
    assert_eq!(
        preparation.tokens_before,
        captured["tokensBefore"].as_u64().unwrap()
    );
    assert_eq!(
        preparation.previous_summary.as_deref(),
        captured["previousSummary"].as_str()
    );
    assert!(captured["summarizeRoles"].as_array().unwrap().is_empty());
    assert!(preparation.messages_to_summarize.is_empty());
    assert_eq!(
        canon_string(&serde_json::to_value(&preparation.turn_prefix_messages).unwrap()),
        captured["turnPrefixJson"].as_str().unwrap()
    );
    assert_eq!(
        roles_of(&preparation.turn_prefix_messages),
        *captured["turnPrefixRoles"].as_array().unwrap()
    );
    assert_eq!(
        file_ops_json(&preparation.file_ops),
        captured["fileOpsJson"].as_str().unwrap()
    );
    assert_eq!(
        canon_string(&serde_json::to_value(preparation.settings).unwrap()),
        canon_string(&captured["settings"])
    );
    let ids = &oracle["system_entry_ids"];
    assert_eq!(Some(system_id.as_str()), ids["system"].as_str());
    assert_eq!(Some(user_id.as_str()), ids["user"].as_str());
    assert_eq!(Some(assistant_id.as_str()), ids["assistant"].as_str());

    // repeated compaction skipped when kept messages still fit
    let mut fixture = Fixture::default();
    let u1 = fixture.message_entry(user_message("user msg 1 (summarized by compaction1)"));
    let a1 = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        "assistant msg 1",
    )));
    let u2 = fixture.message_entry(user_message("user msg 2 - kept by compaction1"));
    let a2 = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        "assistant msg 2",
    )));
    let u3 = fixture.message_entry(user_message("user msg 3 - kept by compaction1"));
    let a3 = fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
        "assistant msg 3",
        mock_usage(5000, 1000),
    )));
    let compaction1 = fixture.compaction_entry("First summary", u2.id().unwrap());
    let u4 = fixture.message_entry(user_message("user msg 4 (new after compaction1)"));
    let a4 = fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
        "assistant msg 4",
        mock_usage(8000, 2000),
    )));
    assert!(prepare_compaction(
        &[u1, a1, u2, a2, u3, a3, compaction1, u4, a4],
        DEFAULT_COMPACTION_SETTINGS
    )
    .is_none());
    assert!(oracle["fits_returns_undefined"].is_null());

    // re-summarize previously kept messages when the window moves past them
    let mut fixture = Fixture::default();
    let u1 = fixture.message_entry(user_message(
        &"user msg 1 (summarized by compaction1)".repeat(4),
    ));
    let a1 = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        &"assistant msg 1".repeat(4),
    )));
    let u2 = fixture.message_entry(user_message(
        &"user msg 2 - kept by compaction1 ".repeat(12),
    ));
    let a2 = fixture.message_entry(AgentMessage::Assistant(assistant_message(
        &"assistant msg 2 ".repeat(12),
    )));
    let u3 = fixture.message_entry(user_message(
        &"user msg 3 - kept by compaction1 ".repeat(12),
    ));
    let a3 = fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
        &"assistant msg 3 ".repeat(12),
        mock_usage(5000, 1000),
    )));
    let compaction2 = fixture.compaction_entry("First summary", u2.id().unwrap());
    let u4 = fixture.message_entry(user_message(
        &"user msg 4 (new after compaction1) ".repeat(12),
    ));
    let a4 = fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
        &"assistant msg 4 ".repeat(12),
        mock_usage(8000, 2000),
    )));
    let preparation = prepare_compaction(
        &[u1, a1, u2, a2, u3, a3, compaction2, u4, a4],
        CompactionSettings {
            keep_recent_tokens: 100,
            ..DEFAULT_COMPACTION_SETTINGS
        },
    )
    .expect("preparation");
    let captured = &oracle["window_moves"];
    assert_eq!(
        preparation.first_kept_entry_id,
        captured["firstKeptEntryId"].as_str().unwrap()
    );
    assert_eq!(
        preparation.tokens_before,
        captured["tokensBefore"].as_u64().unwrap()
    );
    assert_eq!(
        preparation.previous_summary.as_deref(),
        captured["previousSummary"].as_str()
    );
    assert_eq!(
        extract_text(&preparation.messages_to_summarize),
        captured["summarizedText"].as_str().unwrap()
    );
    assert_eq!(
        extract_text(&preparation.turn_prefix_messages),
        captured["turnPrefixText"].as_str().unwrap()
    );
    assert_eq!(
        roles_of(&preparation.messages_to_summarize),
        *captured["summarizeRoles"].as_array().unwrap()
    );
    assert_eq!(
        file_ops_json(&preparation.file_ops),
        captured["fileOpsJson"].as_str().unwrap()
    );

    // trailing compaction and empty session
    let mut fixture = Fixture::default();
    let t_u1 = fixture.message_entry(user_message("only"));
    let t_compaction = fixture.compaction_entry("Fresh summary", t_u1.id().unwrap());
    assert!(prepare_compaction(&[t_u1, t_compaction], DEFAULT_COMPACTION_SETTINGS).is_none());
    assert!(oracle["trailing_compaction_undefined"].is_null());
    assert!(prepare_compaction(&[], DEFAULT_COMPACTION_SETTINGS).is_none());
    assert!(oracle["empty_session_undefined"].is_null());

    // previous compaction file details accumulate into the new preparation
    let mut fixture = Fixture::default();
    let f_u1 = fixture.message_entry(user_message("history question"));
    let f_a1 = fixture.message_entry(AgentMessage::Assistant(AssistantMessage {
        content: vec![
            text_block("working"),
            tool_call_block("read", json!({"path": "from-calls.txt"})),
        ],
        usage: mock_usage_full(50, 20, 900, 0),
        ..assistant_message("unused")
    }));
    let f_compaction = fixture.compaction_entry_with(
        "History summary",
        f_u1.id().unwrap(),
        Some(json!({
            "readFiles": ["prev-read-1.txt", "prev-read-2.txt", "both.txt"],
            "modifiedFiles": ["prev-modified.txt", "both.txt"]
        })),
        false,
    );
    let f_u2 = fixture.message_entry(user_message(&"recent question ".repeat(8)));
    let f_a2 = fixture.message_entry(AgentMessage::Assistant(AssistantMessage {
        content: vec![
            text_block(&"more work ".repeat(6)),
            tool_call_block("write", json!({"path": "written-now.txt"})),
        ],
        usage: mock_usage(8000, 2000),
        ..assistant_message("unused")
    }));
    let f_u1_id = f_u1.id().unwrap().to_string();
    let f_a1_id = f_a1.id().unwrap().to_string();
    let f_compaction_id = f_compaction.id().unwrap().to_string();
    let f_u2_id = f_u2.id().unwrap().to_string();
    let f_a2_id = f_a2.id().unwrap().to_string();
    let preparation = prepare_compaction(
        &[f_u1, f_a1, f_compaction, f_u2, f_a2],
        CompactionSettings {
            keep_recent_tokens: 40,
            ..DEFAULT_COMPACTION_SETTINGS
        },
    )
    .expect("preparation");
    let captured = &oracle["file_details_accumulate"];
    assert_eq!(
        preparation.first_kept_entry_id,
        captured["firstKeptEntryId"].as_str().unwrap()
    );
    assert_eq!(
        preparation.tokens_before,
        captured["tokensBefore"].as_u64().unwrap()
    );
    assert_eq!(
        preparation.previous_summary.as_deref(),
        captured["previousSummary"].as_str()
    );
    assert_eq!(
        roles_of(&preparation.messages_to_summarize),
        *captured["summarizeRoles"].as_array().unwrap()
    );
    assert_eq!(
        file_ops_json(&preparation.file_ops),
        captured["fileOpsJson"].as_str().unwrap()
    );
    let ids = &oracle["file_details_entry_ids"];
    assert_eq!(Some(f_u1_id.as_str()), ids["u1"].as_str());
    assert_eq!(Some(f_a1_id.as_str()), ids["a1"].as_str());
    assert_eq!(Some(f_compaction_id.as_str()), ids["compaction"].as_str());
    assert_eq!(Some(f_u2_id.as_str()), ids["u2"].as_str());
    assert_eq!(Some(f_a2_id.as_str()), ids["a2"].as_str());
}

#[tokio::test]
async fn oracle_summarization_prompts_match() {
    let oracle = oracle()["summarizationPrompts"].clone();
    let run = |name: &str| oracle[name]["calls"].clone();

    // base: caller routing session, default prompt
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    generate_summary_with_usage(SummaryRequest {
        session_id: Some("route-1"),
        ..summary_request(
            &summary_messages(),
            &test_model(),
            2000,
            Some("test-key"),
            scripted_stream_fn(vec![text_response("## Goal\nTest summary")], &calls),
            &mut callbacks,
        )
    })
    .await
    .expect("summary");
    let logged = calls_of(&calls);
    assert_eq!(logged.len(), 1);
    assert_eq!(
        context_json(&logged[0].0),
        canon_string(&run("base")[0]["context"])
    );
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&run("base")[0]["options"])
    );

    // custom instructions appended
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    generate_summary_with_usage(SummaryRequest {
        custom_instructions: Some("Focus on tests"),
        ..summary_request(
            &summary_messages(),
            &test_model(),
            2000,
            Some("test-key"),
            scripted_stream_fn(vec![text_response("ok")], &calls),
            &mut callbacks,
        )
    })
    .await
    .expect("summary");
    let logged = calls_of(&calls);
    assert_eq!(
        context_json(&logged[0].0),
        canon_string(&run("custom_instructions")[0]["context"])
    );
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&run("custom_instructions")[0]["options"])
    );

    // previous summary switches to the update prompt
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    generate_summary_with_usage(SummaryRequest {
        previous_summary: Some("previous checkpoint"),
        ..summary_request(
            &summary_messages(),
            &test_model(),
            2000,
            Some("test-key"),
            scripted_stream_fn(vec![text_response("merged")], &calls),
            &mut callbacks,
        )
    })
    .await
    .expect("summary");
    let logged = calls_of(&calls);
    assert_eq!(
        context_json(&logged[0].0),
        canon_string(&run("previous_summary")[0]["context"])
    );
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&run("previous_summary")[0]["options"])
    );

    // thinking levels and model capability
    for (name, reasoning, thinking) in [
        (
            "thinking_medium_reasoning_model",
            true,
            Some(ThinkingLevel::Medium),
        ),
        (
            "thinking_off_reasoning_model",
            true,
            Some(ThinkingLevel::Off),
        ),
        (
            "thinking_medium_non_reasoning_model",
            false,
            Some(ThinkingLevel::Medium),
        ),
    ] {
        let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
        let mut callbacks = RetryCallbacks::default();
        generate_summary_with_usage(SummaryRequest {
            thinking_level: thinking,
            ..summary_request(
                &summary_messages(),
                &test_model_with(reasoning, 8192),
                2000,
                Some("test-key"),
                scripted_stream_fn(vec![text_response("ok")], &calls),
                &mut callbacks,
            )
        })
        .await
        .expect("summary");
        let logged = calls_of(&calls);
        assert_eq!(
            context_json(&logged[0].0),
            canon_string(&run(name)[0]["context"]),
            "{name}"
        );
        assert_eq!(
            options_json_of(&logged[0].1),
            canon_string(&run(name)[0]["options"]),
            "{name}"
        );
    }

    // max tokens clamped to the model cap (min(0.8 * reserve, maxTokens))
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    generate_summary_with_usage(summary_request(
        &summary_messages(),
        &test_model_with(false, 128_000),
        500_000,
        Some("test-key"),
        scripted_stream_fn(vec![text_response("ok")], &calls),
        &mut callbacks,
    ))
    .await
    .expect("summary");
    let logged = calls_of(&calls);
    assert_eq!(
        context_json(&logged[0].0),
        canon_string(&run("max_tokens_clamped_to_model")[0]["context"])
    );
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&run("max_tokens_clamped_to_model")[0]["options"])
    );

    // headers and env ride through
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let mut headers = std::collections::BTreeMap::new();
    headers.insert("x-trace".to_string(), Some("t1".to_string()));
    let mut env = std::collections::BTreeMap::new();
    env.insert("REGION".to_string(), "eu".to_string());
    generate_summary_with_usage(SummaryRequest {
        headers: Some(&headers),
        env: Some(&env),
        ..summary_request(
            &summary_messages(),
            &test_model(),
            2000,
            Some("test-key"),
            scripted_stream_fn(vec![text_response("ok")], &calls),
            &mut callbacks,
        )
    })
    .await
    .expect("summary");
    let logged = calls_of(&calls);
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&run("headers_and_env")[0]["options"])
    );
}

#[test]
fn oracle_fresh_routing_capture_matches() {
    let fresh = oracle()["summarizationPrompts"]["fresh_routing_session"]["freshRouting"].clone();
    assert_eq!(fresh["count"].as_u64().unwrap(), 2);
    assert!(fresh["distinct"].as_bool().unwrap());
    assert!(fresh["allV7"].as_bool().unwrap());
    // The port's fresh ids satisfy the same shape (see
    // fresh_routing_ids_differ_between_calls).
}

#[tokio::test]
async fn oracle_complete_summarization_option_preservation_matches() {
    let oracle = oracle()["completeSummarizationOptionPreservation"]["call"].clone();
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let stream_fn = scripted_stream_fn(vec![text_response("ok")], &calls);
    let mut callbacks = RetryCallbacks::default();

    complete_summarization(
        &test_model(),
        crate::ai::transcript::normalize_context(&crate::ai::transcript::Context {
            system_prompt: Some("Summarize".to_string()),
            messages: Vec::new(),
            tools: None,
        }),
        SimpleStreamOptions {
            stream: crate::ai::types::StreamOptions {
                session_id: Some("current-routing-session".to_string()),
                cache_retention: Some(CacheRetention::Short),
                max_tokens: Some(999),
                ..crate::ai::types::StreamOptions::default()
            },
            tool_choice: Some(ToolChoice::Auto),
            ..SimpleStreamOptions::default()
        },
        stream_fn,
        None,
        &mut callbacks,
    )
    .await;

    let (context, options) = &calls_of(&calls)[0];
    assert_eq!(canon_string(context), canon_string(&oracle["context"]));
    assert_eq!(canon_string(options), canon_string(&oracle["options"]));
}

#[tokio::test]
async fn oracle_compact_results_match() {
    let oracle = oracle();

    // ---- plain compaction ----
    let plain = oracle["compactPlain"].clone();
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let mut file_ops = FileOperations::new();
    extract_file_ops_from_message(
        &AgentMessage::Assistant(AssistantMessage {
            content: vec![
                tool_call_block("read", json!({"path": "b.txt"})),
                tool_call_block("edit", json!({"path": "a.txt"})),
            ],
            ..text_response("x")
        }),
        &mut file_ops,
    );
    let preparation = CompactionPreparation {
        first_kept_entry_id: "entry-keep".to_string(),
        messages_to_summarize: vec![
            user_message("history"),
            AgentMessage::Assistant(assistant_message("reply")),
        ],
        turn_prefix_messages: Vec::new(),
        is_split_turn: false,
        tokens_before: 4321,
        previous_summary: None,
        file_ops,
        settings: default_settings(),
    };
    let result = compact(
        preparation,
        compact_options(
            &test_model(),
            scripted_stream_fn(vec![text_response("## Goal\nPlain summary")], &calls),
            &mut callbacks,
        ),
    )
    .await
    .expect("compact");
    let logged = calls_of(&calls);
    assert_eq!(logged.len(), 1);
    assert_eq!(
        context_json(&logged[0].0),
        canon_string(&plain["calls"][0]["context"])
    );
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&plain["calls"][0]["options"])
    );
    assert_eq!(
        canon_string(&serde_json::to_value(&result).unwrap()),
        plain["resultJson"].as_str().unwrap()
    );

    // ---- split turn with usage combination ----
    let split = oracle["compactSplitTurn"].clone();
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let history_usage = Usage {
        cache_write_1h: Some(12),
        ..mock_usage(100, 40)
    };
    let prefix_usage = Usage {
        reasoning: Some(7),
        ..mock_usage(30, 25)
    };
    let responses: Arc<Vec<AssistantMessage>> = Arc::new(vec![
        text_response_with_usage("## Goal\nHistory summary", history_usage),
        text_response_with_usage("## Goal\nPrefix summary", prefix_usage),
    ]);
    let responses_for_closure = Arc::clone(&responses);
    let calls_for_closure = Arc::clone(&calls);
    let stream_fn: StreamFn = Arc::new(move |_model, context, options| {
        let mut log = calls_for_closure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let response = responses_for_closure
            .get(log.len())
            .cloned()
            .expect("scripted response");
        log.push(CapturedCall {
            context: scrub_context(&context),
            options: scrub_options(&options),
        });
        drop(log);
        Box::pin(async move { response })
    });
    let preparation = CompactionPreparation {
        first_kept_entry_id: "entry-keep".to_string(),
        messages_to_summarize: vec![user_message("history")],
        turn_prefix_messages: vec![user_message("prefix question")],
        is_split_turn: true,
        tokens_before: 777,
        previous_summary: Some("prior checkpoint".to_string()),
        file_ops: FileOperations::new(),
        settings: default_settings(),
    };
    let result = compact(
        preparation,
        compact_options(&test_model(), Some(stream_fn), &mut callbacks),
    )
    .await
    .expect("compact");
    let logged = calls_of(&calls);
    assert_eq!(logged.len(), 2);
    assert_eq!(
        context_json(&logged[0].0),
        canon_string(&split["calls"][0]["context"])
    );
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&split["calls"][0]["options"])
    );
    assert_eq!(
        context_json(&logged[1].0),
        canon_string(&split["calls"][1]["context"])
    );
    assert_eq!(
        options_json_of(&logged[1].1),
        canon_string(&split["calls"][1]["options"])
    );
    assert_eq!(
        canon_string(&serde_json::to_value(&result).unwrap()),
        split["resultJson"].as_str().unwrap()
    );

    // ---- split turn with empty history ----
    let empty_history = oracle["compactSplitTurnEmptyHistory"].clone();
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let preparation = CompactionPreparation {
        first_kept_entry_id: "entry-keep".to_string(),
        messages_to_summarize: Vec::new(),
        turn_prefix_messages: vec![user_message("prefix question")],
        is_split_turn: true,
        tokens_before: 100,
        previous_summary: Some("previous checkpoint".to_string()),
        file_ops: FileOperations::new(),
        settings: default_settings(),
    };
    let result = compact(
        preparation,
        compact_options(
            &test_model(),
            scripted_stream_fn(vec![text_response("## Goal\nPrefix only")], &calls),
            &mut callbacks,
        ),
    )
    .await
    .expect("compact");
    let logged = calls_of(&calls);
    assert_eq!(logged.len(), 1);
    assert_eq!(
        context_json(&logged[0].0),
        canon_string(&empty_history["calls"][0]["context"])
    );
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&empty_history["calls"][0]["options"])
    );
    assert_eq!(
        canon_string(&serde_json::to_value(&result).unwrap()),
        empty_history["resultJson"].as_str().unwrap()
    );

    // ---- turn prefix output budget ----
    let prefix = oracle["turnPrefixOptions"].clone();
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let preparation = CompactionPreparation {
        first_kept_entry_id: "entry-keep".to_string(),
        messages_to_summarize: Vec::new(),
        turn_prefix_messages: vec![user_message("prefix question")],
        is_split_turn: true,
        tokens_before: 100,
        previous_summary: None,
        file_ops: FileOperations::new(),
        settings: CompactionSettings {
            enabled: true,
            reserve_tokens: 2001,
            keep_recent_tokens: 20,
        },
    };
    compact(
        preparation,
        compact_options(
            &test_model(),
            scripted_stream_fn(vec![text_response("prefix out")], &calls),
            &mut callbacks,
        ),
    )
    .await
    .expect("compact");
    let logged = calls_of(&calls);
    assert_eq!(logged.len(), 1);
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&prefix["options"])
    );
}

#[tokio::test]
async fn oracle_branch_summaries_match() {
    let oracle = oracle()["branchSummary"].clone();

    // default
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let result = generate_branch_summary(
        &branch_entries(),
        branch_options(
            &test_model(),
            scripted_stream_fn(vec![text_response("summary body")], &calls),
            &mut callbacks,
        ),
    )
    .await;
    let logged = calls_of(&calls);
    assert_eq!(logged.len(), 1);
    assert_eq!(
        context_json(&logged[0].0),
        canon_string(&oracle["default"]["calls"][0]["context"])
    );
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&oracle["default"]["calls"][0]["options"])
    );
    assert_eq!(
        canon_string(&serde_json::to_value(&result).unwrap()),
        oracle["default"]["resultJson"].as_str().unwrap()
    );

    // clamped model cap
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    generate_branch_summary(
        &branch_entries(),
        branch_options(
            &test_model_with(false, 1024),
            scripted_stream_fn(vec![text_response("summary body")], &calls),
            &mut callbacks,
        ),
    )
    .await;
    let logged = calls_of(&calls);
    assert_eq!(
        options_json_of(&logged[0].1),
        canon_string(&oracle["clamped"]["calls"][0]["options"])
    );

    // tool call rejection
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let result = generate_branch_summary(
        &branch_entries(),
        branch_options(
            &test_model(),
            scripted_stream_fn(vec![tool_call_response()], &calls),
            &mut callbacks,
        ),
    )
    .await;
    assert_eq!(
        canon_string(&serde_json::to_value(&result).unwrap()),
        oracle["tool_call"]["resultJson"].as_str().unwrap()
    );

    // length rejection
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let result = generate_branch_summary(
        &branch_entries(),
        branch_options(
            &test_model(),
            scripted_stream_fn(
                vec![AssistantMessage {
                    stop_reason: StopReason::Length,
                    ..text_response("partial")
                }],
                &calls,
            ),
            &mut callbacks,
        ),
    )
    .await;
    assert_eq!(
        canon_string(&serde_json::to_value(&result).unwrap()),
        oracle["length"]["resultJson"].as_str().unwrap()
    );

    // aborted
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let result = generate_branch_summary(
        &branch_entries(),
        branch_options(
            &test_model(),
            scripted_stream_fn(
                vec![AssistantMessage {
                    stop_reason: StopReason::Aborted,
                    content: Vec::new(),
                    ..text_response("ignored")
                }],
                &calls,
            ),
            &mut callbacks,
        ),
    )
    .await;
    assert_eq!(
        canon_string(&serde_json::to_value(&result).unwrap()),
        oracle["aborted"]["resultJson"].as_str().unwrap()
    );

    // empty entries
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let result = generate_branch_summary(
        &[SessionEntry::Other],
        branch_options(
            &test_model(),
            scripted_stream_fn(vec![text_response("never called")], &calls),
            &mut callbacks,
        ),
    )
    .await;
    assert!(calls_of(&calls).is_empty());
    assert_eq!(
        canon_string(&serde_json::to_value(&result).unwrap()),
        oracle["empty"]["resultJson"].as_str().unwrap()
    );

    // custom instructions appended / replacing
    for (name, custom, replace) in [
        ("custom_instructions", Some("Talk about the API"), false),
        ("replace_instructions", Some("Only list files"), true),
    ] {
        let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
        let mut callbacks = RetryCallbacks::default();
        generate_branch_summary(
            &branch_entries(),
            GenerateBranchSummaryOptions {
                custom_instructions: custom,
                replace_instructions: replace,
                ..branch_options(
                    &test_model(),
                    scripted_stream_fn(vec![text_response("unused")], &calls),
                    &mut callbacks,
                )
            },
        )
        .await;
        let logged = calls_of(&calls);
        assert_eq!(
            context_json(&logged[0].0),
            canon_string(&oracle[name]["calls"][0]["context"]),
            "{name}"
        );
        assert_eq!(
            options_json_of(&logged[0].1),
            canon_string(&oracle[name]["calls"][0]["options"]),
            "{name}"
        );
    }

    // branch summary details accumulate into file ops
    let file_details_entries = vec![
        SessionEntry::Message {
            id: "branch-user".to_string(),
            parent_id: None,
            message: user_message_at("Abandoned request", 1),
        },
        SessionEntry::BranchSummary {
            id: "branch-1".to_string(),
            parent_id: None,
            from_id: "older".to_string(),
            summary: "Earlier exploration notes".to_string(),
            details: Some(json!({
                "readFiles": ["kept-read.txt", "shared.txt"],
                "modifiedFiles": ["kept-modified.txt", "shared.txt"]
            })),
            from_hook: false,
            timestamp: STAMP.to_string(),
        },
        SessionEntry::Message {
            id: "branch-2".to_string(),
            parent_id: Some("branch-1".to_string()),
            message: AgentMessage::Assistant(AssistantMessage {
                content: vec![tool_call_block("edit", json!({"path": "edited-now.txt"}))],
                api: "a".to_string(),
                provider: "p".to_string(),
                model: "m".to_string(),
                usage: mock_usage(1, 1),
                stop_reason: StopReason::ToolUse,
                timestamp: 5,
                ..text_response("unused")
            }),
        },
    ];
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut callbacks = RetryCallbacks::default();
    let result = generate_branch_summary(
        &file_details_entries,
        branch_options(
            &test_model(),
            scripted_stream_fn(vec![text_response("branch summary text")], &calls),
            &mut callbacks,
        ),
    )
    .await;
    assert_eq!(
        canon_string(&serde_json::to_value(&result).unwrap()),
        oracle["file_details"]["resultJson"].as_str().unwrap()
    );
}

#[test]
fn oracle_prepare_branch_entries_matches() {
    let oracle = oracle()["prepareBranchEntries"].clone();

    let mut fixture = Fixture::default();
    let budget_entries = vec![
        fixture.message_entry(user_message(&"x".repeat(400))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &"y".repeat(400),
            mock_usage(0, 900),
        ))),
        fixture.message_entry(user_message(&"x".repeat(400))),
        fixture.message_entry(AgentMessage::Assistant(assistant_message_with_usage(
            &"y".repeat(400),
            mock_usage(0, 100),
        ))),
    ];

    let unlimited = prepare_branch_entries(&budget_entries, 0);
    assert_eq!(
        roles_of(&unlimited.messages),
        *oracle["unlimited"]["roles"].as_array().unwrap()
    );
    assert_eq!(
        unlimited.total_tokens,
        oracle["unlimited"]["totalTokens"].as_u64().unwrap()
    );

    let keeps_recent = prepare_branch_entries(&budget_entries, 250);
    assert_eq!(
        roles_of(&keeps_recent.messages),
        *oracle["keeps_recent"]["roles"].as_array().unwrap()
    );
    assert_eq!(
        keeps_recent.total_tokens,
        oracle["keeps_recent"]["totalTokens"].as_u64().unwrap()
    );

    // Summary entries squeeze past the budget when it is at least 90% free.
    fixture.reset();
    let squeezed = vec![
        fixture.message_entry(user_message(&"x".repeat(400))),
        fixture.compaction_entry(&"s".repeat(300), "t-0"),
    ];
    let summary_squeeze = prepare_branch_entries(&squeezed, 50);
    assert_eq!(
        roles_of(&summary_squeeze.messages),
        *oracle["summary_squeeze"]["roles"].as_array().unwrap()
    );
    assert_eq!(
        summary_squeeze.total_tokens,
        oracle["summary_squeeze"]["totalTokens"].as_u64().unwrap()
    );

    // File ops accumulate from tool calls; compaction details do NOT feed the
    // branch pass (only branch_summary details do, per upstream).
    fixture.reset();
    let with_tool_calls = vec![
        fixture.message_entry(AgentMessage::Assistant(AssistantMessage {
            content: vec![
                tool_call_block("read", json!({"path": "budget-read.txt"})),
                tool_call_block("write", json!({"path": "budget-write.txt"})),
            ],
            api: "a".to_string(),
            provider: "p".to_string(),
            model: "m".to_string(),
            usage: mock_usage(1, 1),
            stop_reason: StopReason::ToolUse,
            timestamp: TS,
            ..text_response("unused")
        })),
        fixture.compaction_entry_with(
            &"c".repeat(300),
            "t-0",
            Some(json!({
                "readFiles": ["detail-read.txt"],
                "modifiedFiles": ["detail-modified.txt"]
            })),
            false,
        ),
    ];
    let file_ops_collected = prepare_branch_entries(&with_tool_calls, 0);
    assert_eq!(
        roles_of(&file_ops_collected.messages),
        *oracle["file_ops_collected"]["roles"].as_array().unwrap()
    );
    assert_eq!(
        file_ops_json(&file_ops_collected.file_ops),
        oracle["file_ops_collected"]["fileOpsJson"]
            .as_str()
            .unwrap()
    );
}
