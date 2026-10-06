//! Byte oracle for the agent-session delta slice: the `usage` and
//! `context_edit` session entry kinds, their append JSONL bytes (key order
//! included), the `appendContextEdit` error messages, the session projection
//! over edits, and the compaction `null` `firstKeptEntryId` self-reference.
//!
//! Captured from the verbatim upstream HEAD session-manager
//! (sha256 `450d82c529933214e088b8422f00061815e617f352f6c834689787a314064bff`)
//! by `tests/fixtures/agent_session_delta_oracle/session-entries/capture_delta.mjs`
//! with the shared deterministic id seams (entry ids `00000001`-style). The
//! Rust replay below mirrors each scenario operation by operation.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::{StopReason, Usage, UsageCost};
use crate::ai::types::{AssistantBlock, StringOrBlocks, TextContent, TextOrImageBlock};
use crate::coding_agent::session_manager::test_id_seam;
use crate::coding_agent::session_manager::{SessionEntry, SessionManager};

/// The captured oracle.
#[derive(Deserialize)]
struct DeltaOracle {
    #[serde(rename = "usage_entry_json")]
    usage_entry_json: Vec<String>,
    #[serde(rename = "usage_entry_raw")]
    usage_entry_raw: Vec<String>,
    #[serde(rename = "context_edit_json")]
    context_edit_json: Vec<String>,
    #[serde(rename = "context_edit_errors")]
    context_edit_errors: Value,
    #[serde(rename = "projection_grid")]
    projection_grid: Value,
    #[serde(rename = "context_matches_projection")]
    context_matches_projection: bool,
    #[serde(rename = "compaction_self_kept")]
    compaction_self_kept: Value,
}

fn oracle() -> DeltaOracle {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/agent_session_delta_oracle/session-entries/agent_session_delta_oracle.json"
    ))
    .expect("delta oracle json")
}

/// Replace every `timestamp` string (ISO) with the shared scrub constant,
/// preserving key order (the crate's serde_json keeps insertion order).
fn mask_timestamp(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(mask_timestamp),
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if key == "timestamp" && item.is_string() {
                    *item = Value::String("1970-01-01T00:00:00.000Z".to_string());
                } else {
                    mask_timestamp(item);
                }
            }
        }
        _ => {}
    }
}

/// Serialize one entry with its `timestamp` value masked as `<ts>`,
/// preserving key order (the port's append literal order).
fn entry_wire_with_ts(entry: &SessionEntry) -> String {
    let mut value = serde_json::to_value(entry).expect("entry json");
    if let Value::Object(map) = &mut value {
        if let Some(item) = map.get_mut("timestamp") {
            *item = Value::String("<ts>".to_string());
        }
    }
    value.to_string()
}

/// Key-sorted canon of one entry, matching the capture driver's `canon`
/// (BTreeMap ordering via serde_json, timestamp scrubbed).
fn canon_entry(entry: &SessionEntry) -> String {
    let mut value = serde_json::to_value(entry).expect("entry json");
    mask_timestamp(&mut value);
    json_bst_canon(&value).to_string()
}

fn json_bst_canon(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(json_bst_canon).collect()),
        Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, Value> =
                std::collections::BTreeMap::new();
            for (key, item) in map {
                sorted.insert(key.clone(), json_bst_canon(item));
            }
            serde_json::to_value(sorted).expect("map")
        }
        other => other.clone(),
    }
}

fn delta_usage(input: u64, output: u64, cost_total: f64) -> Usage {
    Usage {
        input,
        output,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output,
        cost: UsageCost {
            input: cost_total / 2.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: cost_total,
        },
    }
}

fn delta_user(text: &str) -> AgentMessage {
    AgentMessage::User(crate::ai::types::message::UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp: 1,
    })
}

fn delta_assistant(text: &str) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-test".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: delta_usage(1, 1, 0.0),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1,
    })
}

fn delta_tool_result() -> AgentMessage {
    AgentMessage::ToolResult(crate::ai::types::message::ToolResultMessage {
        tool_call_id: "t1".to_string(),
        tool_name: "read".to_string(),
        content: vec![TextOrImageBlock::Text(TextContent {
            text: "out".to_string(),
            text_signature: None,
        })],
        details: None,
        usage: None,
        is_error: false,
        timestamp: 1,
    })
}

fn edit_text(text: &str) -> Option<crate::coding_agent::session_manager::ContextEditReplacement> {
    Some(
        crate::coding_agent::session_manager::ContextEditReplacement {
            content: crate::coding_agent::session_manager::ContextEditableContent::Text(
                text.to_string(),
            ),
        },
    )
}

#[test]
fn delta_oracle_usage_entry_lines() {
    let oracle = oracle();
    test_id_seam::reset();
    let mut manager = SessionManager::in_memory("<root>", None, None).unwrap();
    manager
        .append_usage(
            "cache_warm",
            "anthropic",
            "claude-test",
            delta_usage(10, 5, 0.5),
            None,
        )
        .unwrap();
    manager
        .append_usage(
            "cache_warm",
            "anthropic",
            "claude-test",
            delta_usage(1, 2, 0.25),
            Some("warm note"),
        )
        .unwrap();
    manager
        .append_usage(
            "cache_warm",
            "anthropic",
            "claude-test",
            delta_usage(1, 1, 0.1),
            Some(""),
        )
        .unwrap();

    let entries = manager.get_entries();
    assert_eq!(entries.len(), oracle.usage_entry_json.len());
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(
            canon_entry(entry),
            oracle.usage_entry_json[index],
            "usage entry canon at {index}"
        );
    }
    // Wire key order: type, id, parentId, timestamp, kind, provider, model,
    // usage, note (note absent for empty/None). Compared field-positionally
    // because the scrubbed capture normalizes the timestamp value.
    let raw_first = entry_wire_with_ts(&entries[0]);
    assert_eq!(
        raw_first,
        r#"{"type":"usage","id":"00000001","parentId":null,"timestamp":"<ts>","kind":"cache_warm","provider":"anthropic","model":"claude-test","usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{"input":0.25,"output":0,"cacheRead":0,"cacheWrite":0,"total":0.5}}}"#
    );
    let raw_note = entry_wire_with_ts(&entries[1]);
    assert_eq!(
        raw_note,
        r#"{"type":"usage","id":"00000002","parentId":"00000001","timestamp":"<ts>","kind":"cache_warm","provider":"anthropic","model":"claude-test","usage":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":3,"cost":{"input":0.125,"output":0,"cacheRead":0,"cacheWrite":0,"total":0.25}},"note":"warm note"}"#
    );
    let raw_empty_note = entry_wire_with_ts(&entries[2]);
    assert!(
        !raw_empty_note.contains("\"note\""),
        "empty note is absent: {raw_empty_note}"
    );
    // The oracle's raw lines agree with the wire shape after the shared
    // timestamp scrub (the exact byte expectations above are the pinned
    // form).
    for (index, entry) in entries.iter().enumerate() {
        let oracle_raw = oracle.usage_entry_raw[index].replace(
            "\"timestamp\":\"1970-01-01T00:00:00.000Z\"",
            "\"timestamp\":\"<ts>\"",
        );
        assert_eq!(entry_wire_with_ts(entry), oracle_raw, "raw line at {index}");
    }
    test_id_seam::disable();
}

#[test]
fn delta_oracle_context_edit_lines_and_errors() {
    let oracle = oracle();
    test_id_seam::reset();
    let mut manager = SessionManager::in_memory("<root>", None, None).unwrap();
    let user = manager.append_message(delta_user("first")).unwrap();
    let assistant = manager.append_message(delta_assistant("answer")).unwrap();
    let tool_result = manager.append_message(delta_tool_result()).unwrap();
    let custom_message = manager
        .append_custom_message_entry(
            "note",
            crate::coding_agent::core::messages::CustomMessageContent::Text(
                "custom body".to_string(),
            ),
            false,
            None,
        )
        .unwrap();
    manager
        .append_model_change("anthropic", "claude-test")
        .unwrap();

    let edited_user = manager
        .append_context_edit(&user, edit_text("edited user"))
        .unwrap();
    let edited_assistant = manager
        .append_context_edit(&assistant, edit_text("edited assistant"))
        .unwrap();
    let edited_tool = manager
        .append_context_edit(&tool_result, edit_text("edited tool"))
        .unwrap();
    let omitted = manager.append_context_edit(&tool_result, None).unwrap();
    let edited_custom = manager
        .append_context_edit(
            &custom_message,
            Some(
                crate::coding_agent::session_manager::ContextEditReplacement {
                    content: crate::coding_agent::session_manager::ContextEditableContent::Blocks(
                        json!([
                            { "type": "text", "text": "custom replacement" }
                        ]),
                    ),
                },
            ),
        )
        .unwrap();

    let edit_ids = [
        edited_user,
        edited_assistant,
        edited_tool,
        omitted,
        edited_custom,
    ];
    for (index, id) in edit_ids.iter().enumerate() {
        let entry = manager.get_entry(id).cloned().expect("edit entry");
        assert_eq!(
            canon_entry(&entry),
            oracle.context_edit_json[index],
            "context edit canon at {index}"
        );
    }

    let mut errors = Vec::new();
    errors.push(
        manager
            .append_context_edit("missing-id", None)
            .expect_err("not found")
            .to_string(),
    );
    manager.branch(&user).unwrap();
    errors.push(
        manager
            .append_context_edit(&tool_result, None)
            .expect_err("off branch")
            .to_string(),
    );
    let thinking = manager.append_thinking_level_change("high").unwrap();
    errors.push(
        manager
            .append_context_edit(&thinking, None)
            .expect_err("not editable")
            .to_string(),
    );
    errors.push(
        manager
            .append_context_edit(
                &user,
                Some(
                    crate::coding_agent::session_manager::ContextEditReplacement {
                        content:
                            crate::coding_agent::session_manager::ContextEditableContent::Blocks(
                                Value::Null,
                            ),
                    },
                ),
            )
            .expect_err("bad content")
            .to_string(),
    );
    let expected_errors: Vec<String> = ["not_found", "off_branch", "not_editable", "bad_content"]
        .iter()
        .map(|key| {
            oracle.context_edit_errors[key]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(errors, expected_errors);
    test_id_seam::disable();
}

#[test]
fn delta_oracle_compaction_self_kept() {
    let oracle = oracle();
    test_id_seam::reset();
    let mut manager = SessionManager::in_memory("<root>", None, None).unwrap();
    manager.append_message(delta_user("u1")).unwrap();
    let id = manager
        .append_compaction(
            "summary so far",
            None,
            1000,
            None,
            Some(false),
            Some(delta_usage(3, 4, 0.2)),
        )
        .unwrap();
    let entry = manager.get_entry(&id).cloned().unwrap();
    let SessionEntry::Compaction(compaction) = &entry else {
        panic!("expected a compaction entry");
    };
    assert!(oracle.compaction_self_kept["firstKeptEntryIdEqualsId"]
        .as_bool()
        .unwrap());
    assert_eq!(compaction.first_kept_entry_id.as_deref(), Some(id.as_str()));
    assert_eq!(
        canon_entry(&entry),
        oracle.compaction_self_kept["canon"],
        "compaction canon"
    );
    test_id_seam::disable();
}

#[test]
fn delta_oracle_projection_grid() {
    let oracle = oracle();
    test_id_seam::reset();
    let mut manager = SessionManager::in_memory("<root>", None, None).unwrap();
    let user = manager.append_message(delta_user("u1")).unwrap();
    manager.append_message(delta_assistant("a1")).unwrap();
    let tool_result = manager.append_message(delta_tool_result()).unwrap();
    let custom_message = manager
        .append_custom_message_entry(
            "note",
            crate::coding_agent::core::messages::CustomMessageContent::Text(
                "custom body".to_string(),
            ),
            false,
            None,
        )
        .unwrap();
    manager.append_message(delta_user("u2")).unwrap();
    manager.append_thinking_level_change("high").unwrap();

    manager
        .append_context_edit(&user, edit_text("u1 edited"))
        .unwrap();
    manager
        .append_context_edit(
            &tool_result,
            Some(
                crate::coding_agent::session_manager::ContextEditReplacement {
                    content: crate::coding_agent::session_manager::ContextEditableContent::Blocks(
                        json!([
                            { "type": "text", "text": "replacement one" },
                            { "type": "image", "data": "Zm9v", "mimeType": "image/png" }
                        ]),
                    ),
                },
            ),
        )
        .unwrap();
    manager
        .append_context_edit(&custom_message, edit_text("custom edited"))
        .unwrap();
    let omitted = manager.append_message(delta_user("u3 omitted")).unwrap();
    manager.append_context_edit(&omitted, None).unwrap();

    let projection = manager.build_session_projection();
    assert_eq!(
        projection.thinking_level,
        oracle.projection_grid["thinkingLevel"].as_str().unwrap()
    );
    assert_eq!(
        serde_json::to_value(&projection.model).unwrap(),
        oracle.projection_grid["model"]
    );
    let shape: Vec<String> = projection
        .entries
        .iter()
        .map(|entry| {
            let kind = match &entry.source_entry {
                SessionEntry::Message(_) => "message",
                SessionEntry::ThinkingLevelChange(_) => "thinking_level_change",
                SessionEntry::ModelChange(_) => "model_change",
                SessionEntry::Usage(_) => "usage",
                SessionEntry::ContextEdit(_) => "context_edit",
                SessionEntry::Compaction(_) => "compaction",
                SessionEntry::BranchSummary(_) => "branch_summary",
                SessionEntry::Custom(_) => "custom",
                SessionEntry::CustomMessage(_) => "custom_message",
                SessionEntry::Label(_) => "label",
                SessionEntry::SessionInfo(_) => "session_info",
                SessionEntry::Unparsed(_) => "unparsed",
            };
            format!("{}:{}", kind, entry.messages.len())
        })
        .collect();
    let expected_shape: Vec<String> = oracle.projection_grid["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            format!(
                "{}:{}",
                entry["type"].as_str().unwrap(),
                entry["messages"].as_array().unwrap().len()
            )
        })
        .collect();
    assert_eq!(shape, expected_shape);

    // The omitted user message projects to nothing.
    assert!(projection.messages.iter().all(|message| match message {
        AgentMessage::User(user) =>
            !matches!(&user.content, StringOrBlocks::Text(text) if text.contains("u3")),
        _ => true,
    }));
    // The edited user message carries the replacement string.
    assert!(projection.messages.iter().any(|message| matches!(
        message,
        AgentMessage::User(user) if matches!(&user.content, StringOrBlocks::Text(text) if text == "u1 edited")
    )));
    // The canonical context matches the projection.
    assert!(oracle.context_matches_projection);
    let context = manager.build_session_context();
    assert_eq!(context.messages, projection.messages);
    assert_eq!(context.thinking_level, projection.thinking_level);
    test_id_seam::disable();
}
