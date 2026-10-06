//! Tests for the [`to_json_event`] projection (upstream
//! `modes/json-event.ts`): byte oracle comparisons against
//! `tests/fixtures/modes_oracle/expected/json_event.jsonl`, captured from the
//! verbatim upstream function under node (`--experimental-strip-types`).
//!
//! Every record in the oracle carries the upstream output for an equivalent
//! input; the port builds the same input over the ported type system (the
//! pi-ai seam already strips the `partial` snapshots upstream strips here)
//! and the serialized outputs must match byte for byte. The internal start snapshot is explicitly stripped at the wire boundary.

use std::sync::OnceLock;

use serde_json::Value;

use super::json_event::{to_json_event, to_json_event_string};
use crate::agent_core::types::AgentMessage;
use crate::ai::models::faux_assistant_message;
use crate::ai::models::FauxContent;
use crate::ai::models::FauxMessageOptions;
use crate::ai::types::content::ImageContent;
use crate::ai::types::content::ToolCall;
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::message::AssistantBlock;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::message::StringOrBlocks;
use crate::ai::types::message::UserMessage;
use crate::ai::types::primitives::StopReason;
use crate::ai::types::primitives::Usage;
use crate::ai::types::primitives::UsageCost;
use crate::ai::TextOrImageBlock;
use crate::coding_agent::agent_session::AgentSessionEvent;

const ORACLE: &str = include_str!("../../../tests/fixtures/modes_oracle/expected/json_event.jsonl");

fn oracle_records() -> &'static Vec<Value> {
    static RECORDS: OnceLock<Vec<Value>> = OnceLock::new();
    RECORDS.get_or_init(|| {
        ORACLE
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()
            .expect("oracle jsonl parses")
    })
}

/// The shared usage fixture (mirrors the ported `Usage` serialization).
fn usage() -> Usage {
    Usage {
        input: 10,
        output: 5,
        cache_read: 2,
        cache_write: 1,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 16,
        cost: UsageCost {
            input: 0.1,
            output: 0.2,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.3,
        },
    }
}

fn tool_call_block() -> ToolCall {
    ToolCall {
        id: "toolu_01".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({ "command": "ls" }),
        thought_signature: None,
        namespace: None,
    }
}

/// An assistant message over the faux provider defaults, matching the oracle
/// fixture (faux api/provider/model, the shared usage and a fixed timestamp).
fn assistant_message(blocks: Vec<AssistantBlock>, usage: Usage) -> AssistantMessage {
    let mut message = faux_assistant_message(
        FauxContent::Blocks(blocks),
        FauxMessageOptions {
            timestamp: Some(1700000000000),
            ..FauxMessageOptions::default()
        },
    );
    message.usage = usage;
    message
}

fn user_message() -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(
            crate::ai::types::content::TextContent {
                text: "hi".to_string(),
                text_signature: None,
            },
        )]),
        timestamp: 1700000000000,
    })
}

fn assistant_agent_message(usage: Usage) -> AgentMessage {
    AgentMessage::Assistant(assistant_message(Vec::new(), usage))
}

fn message_update(message: AgentMessage, event: AssistantMessageEvent) -> AgentSessionEvent {
    AgentSessionEvent::MessageUpdate {
        message,
        assistant_message_event: serde_json::to_value(&event).expect("event serializes"),
    }
}

/// Assert the port output for `event` byte-matches the oracle record.
fn assert_oracle(name: &str, event: &AgentSessionEvent) {
    let record = oracle_records()
        .iter()
        .find(|record| record["name"] == name)
        .unwrap_or_else(|| panic!("oracle record {name} missing"));
    assert!(
        record["ok"].as_bool().unwrap_or(false),
        "oracle case {name} unexpectedly errors"
    );
    let expected = record["out"].as_str().expect("oracle output string");
    let actual = to_json_event_string(event).unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(actual, expected, "byte mismatch for {name}");
}

fn assert_oracle_error(name: &str, event: &AgentSessionEvent) {
    let record = oracle_records()
        .iter()
        .find(|record| record["name"] == name)
        .unwrap_or_else(|| panic!("oracle record {name} missing"));
    let expected = record["error"].as_str().expect("oracle error message");
    let error = to_json_event(event).expect_err("expected projection error");
    assert_eq!(error, expected, "error mismatch for {name}");
}

#[test]
fn passthrough_events_match_oracle() {
    let cases: Vec<(&str, AgentSessionEvent)> = vec![
        ("passthrough_agent_start", AgentSessionEvent::AgentStart),
        (
            "passthrough_agent_end",
            AgentSessionEvent::AgentEnd {
                messages: Vec::new(),
                will_retry: false,
            },
        ),
        ("passthrough_agent_settled", AgentSessionEvent::AgentSettled),
        (
            "passthrough_queue_update",
            AgentSessionEvent::QueueUpdate {
                steering: vec!["a".to_string()],
                follow_up: Vec::new(),
            },
        ),
        ("passthrough_turn_start", AgentSessionEvent::TurnStart),
        (
            "passthrough_message_start",
            AgentSessionEvent::MessageStart {
                message: user_message(),
            },
        ),
        (
            "passthrough_message_end",
            AgentSessionEvent::MessageEnd {
                message: AgentMessage::Assistant(assistant_message(
                    vec![faux_text_block("hello")],
                    usage(),
                )),
            },
        ),
        (
            "passthrough_tool_execution_start",
            AgentSessionEvent::ToolExecutionStart {
                tool_call_id: "toolu_01".to_string(),
                tool_name: "bash".to_string(),
                args: serde_json::json!({ "command": "ls" }),
            },
        ),
        (
            "passthrough_tool_execution_update",
            AgentSessionEvent::ToolExecutionUpdate {
                tool_call_id: "toolu_01".to_string(),
                tool_name: "bash".to_string(),
                args: serde_json::json!({ "command": "ls" }),
                partial_result: serde_json::json!({ "output": "x" }),
            },
        ),
        (
            "passthrough_tool_execution_end",
            AgentSessionEvent::ToolExecutionEnd {
                tool_call_id: "toolu_01".to_string(),
                tool_name: "bash".to_string(),
                result: serde_json::json!({ "output": "done", "exitCode": 0 }),
                is_error: false,
            },
        ),
        (
            "passthrough_compaction_start",
            AgentSessionEvent::CompactionStart {
                reason: crate::coding_agent::agent_session::CompactionReason::Manual,
            },
        ),
        (
            "passthrough_compaction_end",
            AgentSessionEvent::CompactionEnd {
                reason: crate::coding_agent::agent_session::CompactionReason::Threshold,
                result: Some(serde_json::json!({ "summary": "s" })),
                aborted: false,
                will_retry: false,
                error_message: None,
            },
        ),
        (
            "passthrough_entry_appended",
            AgentSessionEvent::EntryAppended {
                entry: entry_appended_session_info(),
            },
        ),
        (
            "passthrough_session_info_changed",
            AgentSessionEvent::SessionInfoChanged {
                name: Some("named".to_string()),
            },
        ),
        (
            "passthrough_thinking_level_changed",
            AgentSessionEvent::ThinkingLevelChanged {
                level: crate::ai::types::ModelThinkingLevel::High,
            },
        ),
        (
            "passthrough_auto_retry_start",
            AgentSessionEvent::AutoRetryStart {
                attempt: 1,
                max_attempts: 3,
                delay_ms: 1000,
                error_message: "boom".to_string(),
            },
        ),
        (
            "passthrough_auto_retry_end",
            AgentSessionEvent::AutoRetryEnd {
                success: true,
                attempt: 2,
                final_error: None,
            },
        ),
        (
            "passthrough_summarization_retry_scheduled",
            AgentSessionEvent::SummarizationRetryScheduled {
                attempt: 1,
                max_attempts: 2,
                delay_ms: 500,
                error_message: "boom".to_string(),
            },
        ),
        (
            "passthrough_summarization_retry_attempt_start",
            AgentSessionEvent::SummarizationRetryAttemptStart {
                source: crate::coding_agent::agent_session::SummarizationRetrySource::BranchSummary,
            },
        ),
        (
            "passthrough_summarization_retry_attempt_start_compaction",
            AgentSessionEvent::SummarizationRetryAttemptStart {
                source: crate::coding_agent::agent_session::SummarizationRetrySource::Compaction {
                    reason: crate::coding_agent::agent_session::CompactionReason::Overflow,
                },
            },
        ),
        (
            "passthrough_summarization_retry_finished",
            AgentSessionEvent::SummarizationRetryFinished,
        ),
        (
            "passthrough_bash_execution_update",
            AgentSessionEvent::BashExecutionUpdate {
                id: Some("b1".to_string()),
                delta: "chunk".to_string(),
            },
        ),
    ];
    for (name, event) in cases {
        assert_oracle(name, &event);
    }
}

fn faux_text_block(text: &str) -> AssistantBlock {
    crate::ai::models::faux_text(text)
}

/// A `session_info` entry matching the oracle fixture
/// (`{"type":"session_info","id","parentId":null,"timestamp","name"}`).
fn entry_appended_session_info() -> crate::coding_agent::session_manager::SessionEntry {
    use crate::coding_agent::session_manager::SessionEntry;
    use crate::coding_agent::session_manager::SessionInfoEntry;
    SessionEntry::SessionInfo(SessionInfoEntry {
        id: "e1".to_string(),
        parent_id: None,
        timestamp: "t".to_string(),
        name: Some("n".to_string()),
    })
}

#[test]
fn message_update_events_match_oracle() {
    let base_usage = usage;
    let cases: Vec<(&str, AssistantMessageEvent, Vec<AssistantBlock>)> = vec![
        (
            "message_update_text_start",
            AssistantMessageEvent::TextStart { content_index: 0 },
            Vec::new(),
        ),
        (
            "message_update_text_delta",
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "he".to_string(),
            },
            Vec::new(),
        ),
        (
            "message_update_text_end",
            AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: "hello".to_string(),
            },
            Vec::new(),
        ),
        (
            "message_update_thinking_start",
            AssistantMessageEvent::ThinkingStart { content_index: 0 },
            Vec::new(),
        ),
        (
            "message_update_thinking_delta",
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "hm".to_string(),
            },
            Vec::new(),
        ),
        (
            "message_update_thinking_end",
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                content: "hmm".to_string(),
            },
            Vec::new(),
        ),
        (
            "message_update_toolcall_delta",
            AssistantMessageEvent::ToolcallDelta {
                content_index: 1,
                delta: "{\"comm".to_string(),
            },
            vec![tool_call_block()]
                .into_iter()
                .map(AssistantBlock::ToolCall)
                .collect(),
        ),
        (
            "message_update_toolcall_end",
            AssistantMessageEvent::ToolcallEnd {
                content_index: 1,
                tool_call: tool_call_block(),
            },
            vec![tool_call_block()]
                .into_iter()
                .map(AssistantBlock::ToolCall)
                .collect(),
        ),
        (
            "message_update_done",
            AssistantMessageEvent::Done {
                reason: crate::ai::types::events::SuccessReason::Stop,
                message: assistant_message(vec![faux_text_block("hello")], base_usage()),
            },
            Vec::new(),
        ),
        (
            "message_update_error",
            AssistantMessageEvent::Error {
                reason: crate::ai::types::events::ErrorReason::Error,
                error: {
                    let mut message = assistant_message(Vec::new(), usage());
                    message.stop_reason = StopReason::Error;
                    message.error_message = Some("boom".to_string());
                    message
                },
            },
            Vec::new(),
        ),
    ];
    for (name, event, content) in cases {
        let message = assistant_message(content, usage());
        assert_oracle(
            name,
            &message_update(AgentMessage::Assistant(message), event),
        );
    }
}

#[test]
fn message_update_toolcall_start_inlines_tool_identity() {
    let usage = usage();
    let content = vec![
        faux_text_block("running"),
        AssistantBlock::ToolCall(tool_call_block()),
    ];
    let message = assistant_message(content, usage);
    assert_oracle(
        "message_update_toolcall_start",
        &message_update(
            AgentMessage::Assistant(message),
            AssistantMessageEvent::ToolcallStart { content_index: 1 },
        ),
    );
}

#[test]
fn message_update_error_paths_match_oracle() {
    assert_oracle_error(
        "error_non_assistant_message",
        &message_update(
            user_message(),
            AssistantMessageEvent::TextStart { content_index: 0 },
        ),
    );

    // toolcall_start pointing at a non-toolCall block.
    let message = assistant_message(vec![faux_text_block("a"), faux_text_block("b")], usage());
    assert_oracle_error(
        "error_toolcall_start_not_tool_call",
        &message_update(
            AgentMessage::Assistant(message),
            AssistantMessageEvent::ToolcallStart { content_index: 1 },
        ),
    );

    // toolcall_start pointing past the end of the content.
    let message = assistant_message(vec![AssistantBlock::ToolCall(tool_call_block())], usage());
    assert_oracle_error(
        "error_toolcall_start_missing_index",
        &message_update(
            AgentMessage::Assistant(message),
            AssistantMessageEvent::ToolcallStart { content_index: 5 },
        ),
    );
}

/// The upstream `JsonAgentSessionEvent` export name survives as the wire
/// value type.
#[test]
fn json_agent_session_event_is_a_value_newtype() {
    let value: Value = serde_json::to_value(&AgentSessionEvent::AgentStart).unwrap();
    let wire = super::json_event::JsonAgentSessionEvent(value);
    assert_eq!(wire.0["type"], "agent_start");
    let _ = assistant_agent_message(usage());
    let _ = ImageContent {
        data: String::new(),
        mime_type: String::new(),
    };
}

#[test]
fn start_snapshot_is_removed_at_wire_boundary() {
    let message = assistant_message(Vec::new(), usage());
    let projected = to_json_event(&message_update(
        AgentMessage::Assistant(message.clone()),
        AssistantMessageEvent::Start { message },
    ))
    .unwrap();
    assert_eq!(
        projected["assistantMessageEvent"],
        serde_json::json!({"type":"start"})
    );
}

#[test]
fn raw_partial_is_removed_and_tool_extra_fields_are_preserved() {
    let event = AgentSessionEvent::MessageUpdate {
        message: AgentMessage::Assistant(assistant_message(
            vec![AssistantBlock::ToolCall(tool_call_block())],
            usage(),
        )),
        assistant_message_event: serde_json::json!({"type":"toolcall_start", "contentIndex":0, "partial":{"content":[AssistantBlock::ToolCall(tool_call_block())]}, "extra":"keep"}),
    };
    let wire = to_json_event_string(&event).unwrap();
    let delta = wire.split("\"assistantMessageEvent\":").nth(1).unwrap();
    assert_eq!(
        &delta[..delta.len() - 1],
        r#"{"type":"toolcall_start","contentIndex":0,"extra":"keep","id":"toolu_01","toolName":"bash"}"#
    );
}

#[test]
fn ordered_ingress_matches_actual_source_with_nested_key_order() {
    use crate::coding_agent::core::model_config::OrderedValue;
    const FIXTURE: &str = include_str!("json_event_ordered_oracle.jsonl");
    let mut count = 0;
    for line in FIXTURE.lines() {
        let record: Value = serde_json::from_str(line).unwrap();
        let input: OrderedValue = serde_json::from_str(record["input"].as_str().unwrap()).unwrap();
        let actual = super::json_event::project_ordered_event(&input);
        if record["ok"] == true {
            assert_eq!(
                actual.unwrap().to_json_string(),
                record["out"].as_str().unwrap(),
                "{}",
                record["name"]
            );
        } else {
            assert_eq!(
                actual.unwrap_err(),
                record["error"].as_str().unwrap(),
                "{}",
                record["name"]
            );
        }
        count += 1;
    }
    assert_eq!(count, 36);
}

// Generated by directly importing upstream json-event.ts, retaining the raw
// input JSON text (including out-of-order integer keys and duplicate keys).
const PROJECTION_ORACLE: &str = include_str!("json_event_projection_oracle.json");

fn assert_json_wire_projection_group(group: &str, expected_count: usize) {
    use crate::agent_core::types::AgentEvent;
    use crate::coding_agent::core::model_config::OrderedValue;

    let report: Value = serde_json::from_str(PROJECTION_ORACLE).unwrap();
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 29);
    let mut checked = 0;
    let mut differences = Vec::new();
    for case in cases.iter().filter(|case| case["group"] == group) {
        let name = case["name"].as_str().unwrap();
        let input = case["input"].as_str().unwrap();
        let event = if group == "chain" {
            let agent_event: AgentEvent = serde_json::from_str(input).unwrap();
            AgentSessionEvent::from(&agent_event)
        } else {
            let raw: Value = serde_json::from_str(input).unwrap();
            AgentSessionEvent::MessageUpdate {
                message: serde_json::from_value(raw["message"].clone()).unwrap(),
                assistant_message_event: raw["assistantMessageEvent"].clone(),
            }
        };
        let before = serde_json::to_string(&event).unwrap();
        let expected = if case["ok"] == true {
            Ok(case["out"].as_str().unwrap().to_owned())
        } else {
            Err(case["error"].as_str().unwrap().to_owned())
        };
        let value_wire = to_json_event(&event).map(|value| serde_json::to_string(&value).unwrap());
        let string_wire = to_json_event_string(&event);
        let ordered: OrderedValue = serde_json::from_str(input).unwrap();
        let ordered_wire =
            super::json_event::project_ordered_event(&ordered).map(|value| value.to_json_string());
        for (path, actual) in [
            ("Value", value_wire),
            ("string", string_wire),
            ("ordered", ordered_wire),
        ] {
            if actual != expected {
                differences.push(format!(
                    "{name} [{path}]\n  actual: {actual:?}\nexpected: {expected:?}"
                ));
            }
        }
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            before,
            "input mutated: {name}"
        );
        if group == "chain" {
            // Exercise the actual production bridge. An OrderedValue-only
            // side path cannot satisfy these arbitrary tool payload cases.
            let agent_event: AgentEvent = serde_json::from_str(input).unwrap();
            let session_event = AgentSessionEvent::from(&agent_event);
            let actual = to_json_event_string(&session_event);
            if actual != expected {
                differences.push(format!("{name} [AgentEvent->AgentSessionEvent]\n  actual: {actual:?}\nexpected: {expected:?}"));
            }
        }
        checked += 1;
    }
    assert_eq!(checked, expected_count, "oracle group {group}");
    assert!(differences.is_empty(), "{}", differences.join("\n\n"));
}

#[test]
fn json_wire_raw_delta_fields_and_partial_authority_match_upstream() {
    assert_json_wire_projection_group("raw", 15);
}

#[test]
fn json_wire_start_extra_fields_stay_in_their_original_slots() {
    assert_json_wire_projection_group("start", 2);
}

#[test]
fn json_wire_terminal_messages_and_tools_preserve_nested_unknown_fields() {
    assert_json_wire_projection_group("terminal", 3);
}

#[test]
fn json_wire_integer_index_keys_are_ordered_recursively() {
    assert_json_wire_projection_group("numeric", 2);
}

#[test]
fn json_wire_tool_payloads_cross_the_actual_agent_session_bridge() {
    assert_json_wire_projection_group("chain", 7);
}

#[path = "json_number_tests.rs"]
mod numbers;

#[path = "json_ingress_tests.rs"]
mod ingress;
